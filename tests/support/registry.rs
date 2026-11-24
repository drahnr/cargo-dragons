use crate::support::TestWorkspace;
use anyhow::{Context, bail, ensure};
use assert_cmd::Command;
use flate2::read::GzDecoder;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    process,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use tempfile::TempDir;

pub const REGISTRY: &str = "dragons-test";
pub const PACKAGE: &str = "dragons-registry-fixture";
pub const VERSION: &str = "0.2.0";
pub const LOGIN_TOKEN: &str = "dragons-test-only-dummy-token";
pub const EXPLICIT_TOKEN: &str = "dragons-test-explicit-dummy-token";
pub const WRONG_TOKEN: &str = "crates-io-only-sentinel-never-forward";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(90);
const IO_TIMEOUT: Duration = Duration::from_secs(5);

pub struct RegistryWorkspace {
    pub workspace: TestWorkspace,
    pub registry: Registry,
    home: TempDir,
}

impl RegistryWorkspace {
    pub fn new() -> anyhow::Result<Self> {
        let registry = Registry::new()?;
        let workspace = TestWorkspace::from_fixture("alternate-registry")?;
        let home = TempDir::new()?;
        // The alternate registry is the only usable registry in this Cargo home.
        fs::write(
            home.path().join("config.toml"),
            format!(
                r#"[registry]
global-credential-providers = ["cargo:token"]

[registries.{REGISTRY}]
index = "{index}"
credential-provider = "cargo:token"


[http]
proxy = ""
timeout = 10

[net]
retry = 0
git-fetch-with-cli = true
"#,
                index = registry.index_url(),
            ),
        )?;
        // Also catch legacy direct git-index lookups that bypass Cargo's source
        // map. Rewrites are child-local and never touch the user's git config.
        fs::write(
            home.path().join("gitconfig"),
            format!(
                "[url \"{}/crates-io/\"]\n\tinsteadOf = https://github.com/rust-lang/crates.io-index\n",
                registry.base_url(),
            ),
        )?;
        Ok(Self {
            workspace,
            registry,
            home,
        })
    }

    fn isolate(&self, cmd: &mut process::Command) {
        // In particular, discard TestWorkspace's offline/crates.io defaults and
        // any real credentials or registry configuration exported by the user.
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy().to_ascii_uppercase();
            if name.starts_with("CARGO_")
                || name.starts_with("__CARGO_TEST_")
                || matches!(name.as_str(), "HTTP_PROXY" | "HTTPS_PROXY" | "ALL_PROXY")
            {
                cmd.env_remove(key);
            }
        }
        cmd.env_remove("CARGO_NET_OFFLINE")
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env("CARGO_HOME", self.home.path())
            .env("CARGO_HTTP_PROXY", "")
            .env("CARGO_TERM_COLOR", "never")
            // Cargo's own test hook redirects even explicit crates.io API calls.
            // This is supported by the pinned Cargo implementation under test.
            .env(
                "__CARGO_TEST_CRATES_IO_URL_DO_NOT_USE_THIS",
                format!("sparse+{}/crates-io/", self.registry.base_url()),
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.home.path().join("gitconfig"))
            .env("GIT_CONFIG_COUNT", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .current_dir(self.workspace.path());
    }

    pub fn command(&self) -> anyhow::Result<Command> {
        let mut cmd = self.workspace.cargo_dragons()?;
        self.isolate(&mut cmd);
        let mut cmd = Command::from_std(cmd);
        cmd.timeout(COMMAND_TIMEOUT);
        Ok(cmd)
    }

    pub fn login(&self) -> anyhow::Result<()> {
        let mut cmd = process::Command::new(option_env!("CARGO").unwrap_or("cargo"));
        self.isolate(&mut cmd);
        cmd.args(["login", "--registry", REGISTRY]);
        let mut cmd = Command::from_std(cmd);
        cmd.timeout(COMMAND_TIMEOUT)
            .write_stdin(format!("{LOGIN_TOKEN}\n"))
            .assert()
            .success();
        let credentials = self.credentials()?;
        ensure!(
            credentials.contains(LOGIN_TOKEN),
            "cargo login did not save the dummy token"
        );
        ensure!(
            !credentials.contains("[registry]"),
            "alternate login must not create crates.io credentials: {credentials}"
        );
        Ok(())
    }

    pub fn append_config(&self, contents: &str) -> anyhow::Result<()> {
        let mut config = fs::OpenOptions::new()
            .append(true)
            .open(self.home.path().join("config.toml"))?;
        writeln!(config, "\n{contents}")?;
        Ok(())
    }

    pub fn credentials(&self) -> anyhow::Result<String> {
        Ok(fs::read_to_string(
            self.home.path().join("credentials.toml"),
        )?)
    }

    pub fn write_credentials(&self, contents: &str) -> anyhow::Result<()> {
        Ok(fs::write(
            self.home.path().join("credentials.toml"),
            contents,
        )?)
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub authorization: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Upload {
    pub name: String,
    pub version: String,
    pub archive: Vec<u8>,
}

#[derive(Default)]
struct State {
    requests: Vec<Request>,
    uploads: Vec<Upload>,
    index: HashMap<String, Vec<String>>,
    errors: Vec<String>,
    revision: usize,
}

pub struct Registry {
    address: SocketAddr,
    state: Arc<Mutex<State>>,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Registry {
    pub fn new() -> anyhow::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let address = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let state = Arc::new(Mutex::new(State::default()));
        let stopping = Arc::new(AtomicBool::new(false));
        let worker = {
            let state = Arc::clone(&state);
            let stopping = Arc::clone(&stopping);
            thread::spawn(move || {
                while !stopping.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if let Err(error) = serve(stream, address, &state) {
                                state.lock().unwrap().errors.push(format!("{error:#}"));
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => {
                            state.lock().unwrap().errors.push(error.to_string());
                            break;
                        }
                    }
                }
            })
        };
        Ok(Self {
            address,
            state,
            stopping,
            worker: Some(worker),
        })
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn index_url(&self) -> String {
        format!("sparse+{}/index/", self.base_url())
    }

    pub fn seed(&self, name: &str, version: &str) {
        let mut state = self.state.lock().unwrap();
        add_index_entry(&mut state, name, version);
    }

    pub fn requests(&self) -> Vec<Request> {
        let state = self.state.lock().unwrap();
        assert!(
            state.errors.is_empty(),
            "registry server errors: {:?}",
            state.errors
        );
        state.requests.clone()
    }

    pub fn uploads(&self) -> Vec<Upload> {
        let state = self.state.lock().unwrap();
        assert!(
            state.errors.is_empty(),
            "registry server errors: {:?}",
            state.errors
        );
        state.uploads.clone()
    }

    pub fn assert_index_requested(&self, name: &str) {
        let expected = format!("/index/{}", index_path(name));
        let requests = self.requests();
        assert!(
            requests
                .iter()
                .any(|request| request.method == "GET" && request.path == expected),
            "expected sparse index request {expected}, got {requests:#?}"
        );
        assert!(
            requests
                .iter()
                .all(|request| !request.path.starts_with("/crates-io/")),
            "alternate registry operation consulted crates.io: {requests:#?}"
        );
    }

    pub fn assert_no_authorization(&self, forbidden: &str) {
        assert!(
            self.requests()
                .iter()
                .all(|request| request.authorization.as_deref() != Some(forbidden)),
            "credentials for another registry were forwarded"
        );
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("registry worker panicked");
        }
    }
}

fn index_path(name: &str) -> String {
    let name = name.to_ascii_lowercase();
    match name.len() {
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => format!("3/{}/{name}", &name[..1]),
        _ => format!("{}/{}/{name}", &name[..2], &name[2..4]),
    }
}

fn add_index_entry(state: &mut State, name: &str, version: &str) {
    assert!(
        !name.is_empty()
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    );
    semver::Version::parse(version).expect("fixture version must be valid semver");
    // No test downloads a crate. A syntactically valid checksum is enough for
    // exact-version queries and Cargo's post-publish availability polling.
    let checksum = "0".repeat(64);
    let entry = format!(
        r#"{{"name":"{name}","vers":"{version}","deps":[],"cksum":"{checksum}","features":{{}},"yanked":false}}"#
    );
    state.index.entry(index_path(name)).or_default().push(entry);
    state.revision += 1;
}

fn read_request(stream: &mut TcpStream) -> anyhow::Result<Request> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().context("missing HTTP method")?.to_owned();
    let path = parts.next().context("missing HTTP path")?.to_owned();
    let mut headers = HashMap::new();
    let mut header_bytes = line.len();
    loop {
        line.clear();
        ensure!(reader.read_line(&mut line)? != 0, "incomplete HTTP headers");
        header_bytes += line.len();
        ensure!(header_bytes < 32 * 1024, "HTTP headers too large");
        if line == "\r\n" || line == "\n" {
            break;
        }
        let (key, value) = line.split_once(':').context("malformed HTTP header")?;
        headers.insert(key.to_ascii_lowercase(), value.trim().to_owned());
    }
    if headers
        .get("expect")
        .is_some_and(|value| value.eq_ignore_ascii_case("100-continue"))
    {
        reader
            .get_mut()
            .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
        reader.get_mut().flush()?;
    }
    ensure!(
        !headers.contains_key("transfer-encoding"),
        "chunked requests are not supported"
    );
    let length: usize = headers
        .get("content-length")
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(0);
    ensure!(length <= 16 * 1024 * 1024, "HTTP body too large");
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Request {
        method,
        path,
        authorization: headers.remove("authorization"),
        body,
    })
}

fn uploaded_crate(body: &[u8]) -> anyhow::Result<Upload> {
    fn length(bytes: &[u8]) -> anyhow::Result<usize> {
        Ok(u32::from_le_bytes(
            bytes
                .get(..4)
                .context("missing upload length")?
                .try_into()?,
        ) as usize)
    }
    let metadata_length = length(body)?;
    let archive_offset = 4 + metadata_length;
    let archive_length = length(
        body.get(archive_offset..)
            .context("missing archive length")?,
    )?;
    let archive = body
        .get(archive_offset + 4..)
        .context("missing crate archive")?;
    ensure!(
        archive.len() == archive_length,
        "incorrect crate archive length"
    );
    let mut tar = tar::Archive::new(GzDecoder::new(archive));
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if path.components().count() == 2
            && path.file_name() == Some(Path::new("Cargo.toml").as_os_str())
        {
            let mut manifest = String::new();
            entry.read_to_string(&mut manifest)?;
            let manifest: toml::Value = toml::from_str(&manifest)?;
            let package = &manifest["package"];
            return Ok(Upload {
                name: package["name"]
                    .as_str()
                    .context("uploaded package name")?
                    .to_owned(),
                version: package["version"]
                    .as_str()
                    .context("uploaded package version")?
                    .to_owned(),
                archive: archive.to_vec(),
            });
        }
    }
    bail!("upload did not contain a normalized Cargo.toml")
}

fn serve(mut stream: TcpStream, address: SocketAddr, state: &Mutex<State>) -> anyhow::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let request = read_request(&mut stream)?;
    let mut state = state.lock().unwrap();
    state.requests.push(request.clone());
    let path = request.path.split('?').next().unwrap();
    let valid_tokens = HashSet::from([LOGIN_TOKEN, EXPLICIT_TOKEN]);
    let authenticated = request
        .authorization
        .as_deref()
        .is_some_and(|token| valid_tokens.contains(token));
    let (status, body) = if request.method == "GET" && path == "/index/config.json" {
        (
            "200 OK",
            format!(
                r#"{{"dl":"http://{address}/download/{{crate}}/{{version}}/download","api":"http://{address}","auth-required":true}}"#
            ),
        )
    } else if path.starts_with("/crates-io/") {
        (
            "404 Not Found",
            r#"{"errors":[{"detail":"public registry disabled in tests"}]}"#.into(),
        )
    } else if !authenticated {
        (
            "401 Unauthorized",
            r#"{"errors":[{"detail":"dummy alternate registry token required"}]}"#.into(),
        )
    } else if request.method == "GET" && path.starts_with("/index/") {
        match state.index.get(path.trim_start_matches("/index/")) {
            Some(entries) => ("200 OK", format!("{}\n", entries.join("\n"))),
            None => ("404 Not Found", String::new()),
        }
    } else if request.method == "PUT" && path == "/api/v1/crates/new" {
        let upload = uploaded_crate(&request.body)?;
        add_index_entry(&mut state, &upload.name, &upload.version);
        state.uploads.push(upload);
        (
            "200 OK",
            r#"{"ok":true,"warnings":{"invalid_categories":[],"invalid_badges":[],"other":[]}}"#
                .into(),
        )
    } else if request.method == "PUT"
        && path.starts_with("/api/v1/crates/")
        && path.ends_with("/owners")
    {
        ("200 OK", r#"{"ok":true,"msg":"owners updated"}"#.into())
    } else {
        (
            "404 Not Found",
            r#"{"errors":[{"detail":"unsupported test registry endpoint"}]}"#.into(),
        )
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-cache\r\nETag: \"{}\"\r\nWWW-Authenticate: Cargo login_url=\"http://{address}/login\"\r\n\r\n{body}",
        body.len(),
        state.revision,
    );
    drop(state);
    stream.write_all(response.as_bytes())?;
    stream.flush()?;
    Ok(())
}
