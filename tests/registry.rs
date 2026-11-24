mod support;

#[path = "support/registry.rs"]
mod registry_support;

use predicates::{prelude::PredicateBooleanExt, str::contains};
use registry_support::{
    EXPLICIT_TOKEN, LOGIN_TOKEN, PACKAGE, REGISTRY, Registry, RegistryWorkspace, VERSION,
    WRONG_TOKEN,
};

const SECOND_REGISTRY: &str = "dragons-second";
const SECOND_PACKAGE: &str = "dragons-private-fixture";
const SECOND_VERSION: &str = "0.1.0";

fn assert_uploaded(workspace: &RegistryWorkspace, token: &str, with_owner: bool) {
    assert_registry_uploaded(&workspace.registry, PACKAGE, VERSION, token, with_owner);
}

fn assert_registry_uploaded(
    registry: &Registry,
    package: &str,
    version: &str,
    token: &str,
    with_owner: bool,
) {
    let uploads = registry.uploads();
    assert_eq!(
        uploads.len(),
        1,
        "expected exactly one upload: {uploads:#?}"
    );
    assert_eq!(uploads[0].name, package);
    assert_eq!(uploads[0].version, version);
    assert!(!uploads[0].archive.is_empty());

    let requests = registry.requests();
    let uploads = Vec::from_iter(
        requests
            .iter()
            .filter(|request| request.method == "PUT" && request.path == "/api/v1/crates/new"),
    );
    assert_eq!(uploads.len(), 1, "requests: {requests:#?}");
    assert_eq!(uploads[0].authorization.as_deref(), Some(token));
    let upload_position = requests
        .iter()
        .position(|request| request.method == "PUT" && request.path == "/api/v1/crates/new")
        .unwrap();
    assert!(
        requests[upload_position + 1..].iter().any(|request| {
            request.method == "GET"
                && request.path.starts_with("/index/")
                && request.path.ends_with(package)
        }),
        "Cargo must poll the alternate sparse index after uploading: {requests:#?}"
    );

    let owner_path = format!("/api/v1/crates/{package}/owners");
    let owners = Vec::from_iter(
        requests
            .iter()
            .filter(|request| request.method == "PUT" && request.path == owner_path),
    );
    assert_eq!(
        owners.len(),
        usize::from(with_owner),
        "requests: {requests:#?}"
    );
    for request in owners {
        assert_eq!(request.authorization.as_deref(), Some(token));
        assert!(String::from_utf8_lossy(&request.body).contains("dummy-owner"));
    }
    registry.assert_index_requested(package);
    registry.assert_no_authorization(WRONG_TOKEN);
    assert!(
        requests
            .iter()
            .all(|request| !request.path.starts_with("/download/")),
        "dependency-free fixture must not download crates"
    );
    assert!(
        requests
            .iter()
            .any(|request| request.method == "GET" && request.path == "/index/config.json"),
        "Cargo must obtain the alternate registry's API configuration"
    );
}

fn publication_command(
    workspace: &RegistryWorkspace,
    operation: &str,
) -> anyhow::Result<assert_cmd::Command> {
    let mut command = workspace.command()?;
    match operation {
        "unleash" | "unleash-dry-run" => {
            command.args([
                "unleash",
                "--ignore-publish",
                "--skip-verify",
                "--owner",
                "dummy-owner",
            ]);
            if operation == "unleash-dry-run" {
                command.arg("--dry-run");
            }
        }
        "add-owner" => {
            command.args(["add-owner", "dummy-owner", "--ignore-publish"]);
        }
        _ => panic!("unknown test operation: {operation}"),
    }
    Ok(command)
}

fn credentials_snapshot(
    workspace: &RegistryWorkspace,
) -> anyhow::Result<Vec<(std::path::PathBuf, Option<Vec<u8>>)>> {
    let command = workspace.command()?;
    let home = command
        .get_envs()
        .find_map(|(key, value)| (key == "CARGO_HOME").then_some(value).flatten())
        .map(std::path::PathBuf::from)
        .expect("registry command must set an isolated CARGO_HOME");
    anyhow::Result::<Vec<_>>::from_iter(["credentials", "credentials.toml"].into_iter().map(
        |name| {
            let path = home.join(name);
            let contents = match std::fs::read(&path) {
                Ok(contents) => Some(contents),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            Ok((path, contents))
        },
    ))
}

fn assert_tokens_redacted(assertion: &assert_cmd::assert::Assert) {
    let output = assertion.get_output();
    for bytes in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(bytes);
        for token in [EXPLICIT_TOKEN, LOGIN_TOKEN, WRONG_TOKEN] {
            assert!(!text.contains(token), "command output exposed a token");
        }
    }
}

fn assert_no_requests(registry: &Registry) {
    let requests = registry.requests();
    assert!(
        requests.is_empty(),
        "authentication must fail before HTTP: {requests:#?}"
    );
    assert!(registry.uploads().is_empty());
}

fn assert_owner_request(registry: &Registry, package: &str, token: &str) {
    let requests = registry.requests();
    let puts = Vec::from_iter(requests.iter().filter(|request| request.method == "PUT"));
    assert_eq!(
        puts.len(),
        1,
        "expected only an owner request: {requests:#?}"
    );
    assert_eq!(puts[0].path, format!("/api/v1/crates/{package}/owners"));
    assert_eq!(puts[0].authorization.as_deref(), Some(token));
    assert!(String::from_utf8_lossy(&puts[0].body).contains("dummy-owner"));
    assert!(registry.uploads().is_empty());
}

fn select_second_registry(workspace: &RegistryWorkspace) -> anyhow::Result<Registry> {
    let registry = Registry::new()?;
    workspace.append_config(&format!(
        "[registries.{SECOND_REGISTRY}]\nindex = {:?}\ncredential-provider = \"cargo:token\"\n",
        registry.index_url()
    ))?;
    let path = workspace
        .workspace
        .path()
        .join(SECOND_PACKAGE)
        .join("Cargo.toml");
    let manifest = std::fs::read_to_string(&path)?;
    anyhow::ensure!(manifest.contains("publish = false"));
    std::fs::write(
        path,
        manifest.replace(
            "publish = false",
            &format!("publish = [\"{SECOND_REGISTRY}\"]"),
        ),
    )?;
    Ok(registry)
}

#[test]
fn alternate_login_without_crates_io_credentials_allows_dry_run() -> anyhow::Result<()> {
    let workspace = RegistryWorkspace::new()?;
    workspace.login()?;
    let credentials = workspace.credentials()?;

    workspace
        .command()?
        .args(["unleash", "--ignore-publish", "--dry-run", "--skip-verify"])
        .assert()
        .success()
        .stderr(contains("Would package").and(contains(PACKAGE)))
        .stderr(contains("Would package (🏜️) dragons-private-fixture").not());

    workspace.registry.assert_index_requested(PACKAGE);
    assert!(workspace.registry.uploads().is_empty());
    assert_eq!(workspace.credentials()?, credentials);
    Ok(())
}

#[test]
fn exact_version_on_alternate_registry_is_not_released_even_by_real_unleash() -> anyhow::Result<()>
{
    let workspace = RegistryWorkspace::new()?;
    workspace.registry.seed(PACKAGE, VERSION);
    workspace.login()?;

    for args in [
        vec!["to-release", "--ignore-publish"],
        vec!["unleash", "--ignore-publish", "--dry-run", "--skip-verify"],
        vec!["unleash", "--ignore-publish", "--skip-verify"],
    ] {
        workspace
            .command()?
            .args(args)
            .assert()
            .success()
            .stdout(contains("already present").and(contains("same version")))
            .stderr(contains("Would package").not());
        assert!(workspace.registry.uploads().is_empty());
    }
    workspace.registry.assert_index_requested(PACKAGE);
    assert!(
        workspace
            .registry
            .requests()
            .iter()
            .all(|request| request.method != "PUT")
    );
    Ok(())
}

#[test]
fn a_different_published_version_does_not_suppress_a_new_release() -> anyhow::Result<()> {
    // Even a compatible greater version matching ^0.2.0 is not this exact version.
    for existing in ["0.1.0", "0.2.1", "0.3.0", "0.2.0-pre.1"] {
        let workspace = RegistryWorkspace::new()?;
        workspace.registry.seed(PACKAGE, existing);
        workspace.login()?;

        workspace
            .command()?
            .args(["to-release", "--ignore-publish"])
            .assert()
            .success()
            .stdout(contains(format!("{PACKAGE} ({VERSION})")));
        workspace
            .command()?
            .args(["unleash", "--ignore-publish", "--dry-run", "--skip-verify"])
            .assert()
            .success()
            .stderr(contains("Would package").and(contains(PACKAGE)));
        assert!(workspace.registry.uploads().is_empty());

        workspace
            .command()?
            .args(["unleash", "--ignore-publish", "--skip-verify"])
            .assert()
            .success();
        assert_uploaded(&workspace, LOGIN_TOKEN, false);
    }
    Ok(())
}

#[test]
fn real_unleash_uses_alternate_login_for_upload_and_owner() -> anyhow::Result<()> {
    let workspace = RegistryWorkspace::new()?;
    workspace.login()?;
    let credentials = workspace.credentials()?;

    workspace
        .command()?
        .args([
            "unleash",
            "--ignore-publish",
            "--skip-verify",
            "--owner",
            "dummy-owner",
        ])
        .assert()
        .success();

    assert_uploaded(&workspace, LOGIN_TOKEN, true);
    assert_eq!(workspace.credentials()?, credentials);

    // A fresh process must observe the entry created by the upload, too.
    workspace
        .command()?
        .args(["to-release", "--ignore-publish"])
        .assert()
        .success()
        .stdout(contains("already present"));
    assert_eq!(workspace.registry.uploads().len(), 1);
    Ok(())
}

#[test]
fn explicit_token_is_scoped_to_the_alternate_registry_without_persisting_it() -> anyhow::Result<()>
{
    let workspace = RegistryWorkspace::new()?;
    let credentials = format!(
        "[registry]\ntoken = \"{WRONG_TOKEN}\"\n\n[registries.unrelated]\ntoken = \"unrelated-dummy-token\"\n"
    );
    workspace.write_credentials(&credentials)?;

    workspace
        .command()?
        .env("CARGO_REGISTRY_TOKEN", WRONG_TOKEN)
        .args([
            "unleash",
            "--ignore-publish",
            "--skip-verify",
            "--token",
            EXPLICIT_TOKEN,
            "--owner",
            "dummy-owner",
        ])
        .assert()
        .success();

    assert_uploaded(&workspace, EXPLICIT_TOKEN, true);
    assert_eq!(workspace.credentials()?, credentials);
    workspace
        .registry
        .assert_no_authorization("unrelated-dummy-token");
    Ok(())
}

#[test]
fn crates_io_environment_token_does_not_override_alternate_login() -> anyhow::Result<()> {
    let workspace = RegistryWorkspace::new()?;
    workspace.login()?;

    workspace
        .command()?
        .env("CARGO_REGISTRY_TOKEN", WRONG_TOKEN)
        .args(["unleash", "--ignore-publish", "--skip-verify"])
        .assert()
        .success();

    assert_uploaded(&workspace, LOGIN_TOKEN, false);
    Ok(())
}

#[test]
fn alternate_environment_token_works_without_login_or_persisting_credentials() -> anyhow::Result<()>
{
    let workspace = RegistryWorkspace::new()?;
    let mut command = workspace.command()?;
    let home = command
        .get_envs()
        .find_map(|(key, value)| (key == "CARGO_HOME").then_some(value).flatten())
        .map(std::path::PathBuf::from)
        .expect("registry command must set an isolated CARGO_HOME");
    let credentials = [home.join("credentials"), home.join("credentials.toml")];
    for path in &credentials {
        assert!(
            !path.exists(),
            "unexpected credentials before publishing: {path:?}"
        );
    }

    command
        .env("CARGO_REGISTRIES_DRAGONS_TEST_TOKEN", LOGIN_TOKEN)
        .args(["unleash", "--ignore-publish", "--skip-verify"])
        .assert()
        .success();

    assert_uploaded(&workspace, LOGIN_TOKEN, false);
    for path in &credentials {
        assert!(
            !path.exists(),
            "environment token persisted credentials: {path:?}"
        );
    }
    Ok(())
}

#[test]
fn missing_alternate_login_names_registry_and_login_command() -> anyhow::Result<()> {
    for operation in ["unleash", "unleash-dry-run", "add-owner"] {
        let workspace = RegistryWorkspace::new()?;
        let credentials = credentials_snapshot(&workspace)?;

        let assertion = publication_command(&workspace, operation)?
            .assert()
            .failure()
            .stderr(contains(REGISTRY))
            .stderr(contains("cargo login --registry dragons-test"));
        assert_tokens_redacted(&assertion);
        assert_no_requests(&workspace.registry);
        assert_eq!(credentials_snapshot(&workspace)?, credentials);
    }
    Ok(())
}

#[test]
fn credentials_for_other_registries_cannot_satisfy_alternate_login() -> anyhow::Result<()> {
    let workspace = RegistryWorkspace::new()?;
    let credentials = format!(
        "[registry]\ntoken = \"{WRONG_TOKEN}\"\n\n[registries.unrelated]\ntoken = \"unrelated-dummy-token\"\n"
    );
    workspace.write_credentials(&credentials)?;

    workspace
        .command()?
        .env("CARGO_REGISTRY_TOKEN", WRONG_TOKEN)
        .args(["unleash", "--ignore-publish", "--skip-verify"])
        .assert()
        .failure()
        .stderr(contains(REGISTRY))
        .stderr(contains("cargo login --registry dragons-test"));

    assert!(workspace.registry.uploads().is_empty());
    workspace.registry.assert_no_authorization(WRONG_TOKEN);
    workspace
        .registry
        .assert_no_authorization("unrelated-dummy-token");
    assert_eq!(workspace.credentials()?, credentials);
    Ok(())
}

#[test]
fn publish_false_is_never_uploaded_even_with_ignore_publish() -> anyhow::Result<()> {
    for ignore_publish in [false, true] {
        let workspace = RegistryWorkspace::new()?;
        workspace.login()?;
        let private_manifest = workspace
            .workspace
            .path()
            .join("dragons-private-fixture/Cargo.toml");
        let before = std::fs::read(&private_manifest)?;

        let mut command = workspace.command()?;
        command.args(["unleash", "--skip-verify"]);
        if ignore_publish {
            command.arg("--ignore-publish");
        }
        command.assert().success();

        if ignore_publish {
            assert_uploaded(&workspace, LOGIN_TOKEN, false);
        } else {
            assert!(workspace.registry.uploads().is_empty());
        }
        assert_eq!(std::fs::read(&private_manifest)?, before);
        assert!(
            workspace
                .registry
                .requests()
                .iter()
                .all(|request| !request.path.contains("dragons-private-fixture"))
        );
    }
    Ok(())
}

#[test]
fn explicit_publish_false_selection_with_ignore_publish_is_a_noop() -> anyhow::Result<()> {
    let workspace = RegistryWorkspace::new()?;
    workspace.login()?;

    workspace
        .command()?
        .args([
            "unleash",
            "--packages",
            "dragons-private-fixture",
            "--ignore-publish",
            "--skip-verify",
        ])
        .assert()
        .success()
        .stdout(contains("No packages selected"));

    assert!(workspace.registry.uploads().is_empty());
    assert!(
        workspace
            .registry
            .requests()
            .iter()
            .all(|request| request.method != "PUT")
    );
    Ok(())
}

fn remove_publish_setting(workspace: &RegistryWorkspace, member: &str) -> anyhow::Result<()> {
    let path = workspace.workspace.path().join(member).join("Cargo.toml");
    let manifest = std::fs::read_to_string(&path)?;
    let without_publish = String::from_iter(
        manifest
            .split_inclusive('\n')
            .filter(|line| !line.starts_with("publish = ")),
    );
    anyhow::ensure!(
        manifest != without_publish,
        "fixture {member} must have a publish setting"
    );
    std::fs::write(path, without_publish)?;
    Ok(())
}

#[test]
fn implicit_crates_io_publication_with_source_replacement_fails_before_requests()
-> anyhow::Result<()> {
    let workspace = RegistryWorkspace::new()?;
    remove_publish_setting(&workspace, PACKAGE)?;
    workspace.append_config("[source.crates-io]\nreplace-with = 'dragons-test'\n")?;

    for dry_run in [false, true] {
        let mut command = workspace.command()?;
        command
            .env("CARGO_REGISTRY_TOKEN", WRONG_TOKEN)
            .args(["unleash", "--skip-verify"]);
        if dry_run {
            command.arg("--dry-run");
        }
        command.assert().failure().stderr(
            contains("source replacement")
                .or(contains("source-replacement"))
                .or(contains("crates-io").and(contains("replaced")))
                .or(contains("explicit registry")),
        );

        let requests = workspace.registry.requests();
        assert!(
            requests.is_empty(),
            "source replacement must be rejected before HTTP: {requests:#?}"
        );
        assert!(workspace.registry.uploads().is_empty());
    }
    Ok(())
}

#[test]
fn standalone_add_owner_uses_saved_alternate_token_not_crates_io_token() -> anyhow::Result<()> {
    let workspace = RegistryWorkspace::new()?;
    workspace.registry.seed(PACKAGE, VERSION);
    workspace.login()?;
    let credentials = workspace.credentials()?;

    workspace
        .command()?
        .env("CARGO_REGISTRY_TOKEN", WRONG_TOKEN)
        .args([
            "add-owner",
            "dummy-owner",
            "--ignore-publish",
            "--packages",
            PACKAGE,
        ])
        .assert()
        .success();

    let requests = workspace.registry.requests();
    let puts = Vec::from_iter(requests.iter().filter(|request| request.method == "PUT"));
    assert_eq!(
        puts.len(),
        1,
        "expected only an owner request: {requests:#?}"
    );
    assert_eq!(puts[0].path, format!("/api/v1/crates/{PACKAGE}/owners"));
    assert_eq!(puts[0].authorization.as_deref(), Some(LOGIN_TOKEN));
    assert!(String::from_utf8_lossy(&puts[0].body).contains("dummy-owner"));
    assert!(
        requests
            .iter()
            .all(|request| !request.path.starts_with("/crates-io/"))
    );
    workspace.registry.assert_no_authorization(WRONG_TOKEN);
    assert!(workspace.registry.uploads().is_empty());
    assert_eq!(workspace.credentials()?, credentials);
    Ok(())
}

#[test]
fn standalone_add_owner_bare_token_for_mixed_registries_still_requires_alternate_login()
-> anyhow::Result<()> {
    let workspace = RegistryWorkspace::new()?;
    remove_publish_setting(&workspace, "dragons-private-fixture")?;

    workspace
        .command()?
        .env("CARGO_REGISTRY_TOKEN", WRONG_TOKEN)
        .args([
            "add-owner",
            "dummy-owner",
            "--ignore-publish",
            "--token",
            EXPLICIT_TOKEN,
        ])
        .assert()
        .failure()
        .stderr(contains(REGISTRY))
        .stderr(contains("cargo login --registry dragons-test"))
        .stderr(contains(EXPLICIT_TOKEN).not())
        .stderr(contains(WRONG_TOKEN).not());

    assert_no_requests(&workspace.registry);
    Ok(())
}

#[test]
fn unleash_bare_token_for_mixed_registries_still_requires_alternate_login() -> anyhow::Result<()> {
    let workspace = RegistryWorkspace::new()?;
    remove_publish_setting(&workspace, "dragons-private-fixture")?;

    for dry_run in [false, true] {
        let mut command = workspace.command()?;
        command.env("CARGO_REGISTRY_TOKEN", WRONG_TOKEN).args([
            "unleash",
            "--ignore-publish",
            "--skip-verify",
            "--token",
            EXPLICIT_TOKEN,
        ]);
        if dry_run {
            command.arg("--dry-run");
        }
        command
            .assert()
            .failure()
            .stderr(contains(REGISTRY))
            .stderr(contains("cargo login --registry dragons-test"))
            .stderr(contains(EXPLICIT_TOKEN).not())
            .stderr(contains(WRONG_TOKEN).not());

        assert_no_requests(&workspace.registry);
    }
    Ok(())
}

#[test]
fn ambiguous_publish_allowlist_is_rejected_instead_of_guessing_a_registry() -> anyhow::Result<()> {
    let workspace = RegistryWorkspace::new()?;
    workspace.login()?;
    let path = workspace
        .workspace
        .path()
        .join("dragons-registry-fixture/Cargo.toml");
    let manifest = std::fs::read_to_string(&path)?;
    std::fs::write(
        path,
        manifest.replace(
            "publish = [\"dragons-test\"]",
            "publish = [\"dragons-test\", \"another-test-registry\"]",
        ),
    )?;

    workspace
        .command()?
        .args(["unleash", "--ignore-publish", "--dry-run", "--skip-verify"])
        .assert()
        .failure()
        .stderr(contains(PACKAGE))
        .stderr(contains(REGISTRY))
        .stderr(contains("another-test-registry"));
    assert!(workspace.registry.uploads().is_empty());
    Ok(())
}

#[test]
fn bare_equals_and_prefixed_tokens_work_without_login_or_persisting_credentials()
-> anyhow::Result<()> {
    for operation in ["unleash", "add-owner"] {
        for token_args in [
            vec!["--token".to_owned(), EXPLICIT_TOKEN.to_owned()],
            vec![format!("--token={EXPLICIT_TOKEN}")],
            vec!["--token".to_owned(), format!("{REGISTRY}:{EXPLICIT_TOKEN}")],
        ] {
            let workspace = RegistryWorkspace::new()?;
            if operation == "add-owner" {
                workspace.registry.seed(PACKAGE, VERSION);
            }
            let credentials = credentials_snapshot(&workspace)?;
            assert!(credentials.iter().all(|(_, contents)| contents.is_none()));

            let assertion = publication_command(&workspace, operation)?
                .args(token_args)
                .assert()
                .success();
            assert_tokens_redacted(&assertion);
            if operation == "unleash" {
                assert_uploaded(&workspace, EXPLICIT_TOKEN, true);
            } else {
                assert_owner_request(&workspace.registry, PACKAGE, EXPLICIT_TOKEN);
            }
            assert_eq!(credentials_snapshot(&workspace)?, credentials);
        }
    }
    Ok(())
}

#[test]
fn bare_token_targets_the_sole_selected_registry_in_a_multi_registry_workspace()
-> anyhow::Result<()> {
    for operation in ["unleash", "add-owner"] {
        let workspace = RegistryWorkspace::new()?;
        let second = select_second_registry(&workspace)?;
        if operation == "add-owner" {
            second.seed(SECOND_PACKAGE, SECOND_VERSION);
        }
        let credentials = credentials_snapshot(&workspace)?;

        let assertion = publication_command(&workspace, operation)?
            .args(["--packages", SECOND_PACKAGE, "--token", EXPLICIT_TOKEN])
            .assert()
            .success();
        assert_tokens_redacted(&assertion);
        assert_no_requests(&workspace.registry);
        if operation == "unleash" {
            assert_registry_uploaded(
                &second,
                SECOND_PACKAGE,
                SECOND_VERSION,
                EXPLICIT_TOKEN,
                true,
            );
        } else {
            assert_owner_request(&second, SECOND_PACKAGE, EXPLICIT_TOKEN);
        }
        assert_eq!(credentials_snapshot(&workspace)?, credentials);
    }
    Ok(())
}

#[test]
fn repeatable_prefixed_tokens_scope_real_uploads_and_owners_to_each_registry() -> anyhow::Result<()>
{
    for operation in ["unleash", "add-owner"] {
        let workspace = RegistryWorkspace::new()?;
        let second = select_second_registry(&workspace)?;
        // Saved credentials and environment tokens must not override either CLI token.
        workspace.write_credentials(&format!(
            "[registries.{REGISTRY}]\ntoken = \"{LOGIN_TOKEN}\"\n\n[registries.{SECOND_REGISTRY}]\ntoken = \"{EXPLICIT_TOKEN}\"\n"
        ))?;
        let credentials = credentials_snapshot(&workspace)?;
        if operation == "add-owner" {
            workspace.registry.seed(PACKAGE, VERSION);
            second.seed(SECOND_PACKAGE, SECOND_VERSION);
        }

        let assertion = publication_command(&workspace, operation)?
            .env("CARGO_REGISTRIES_DRAGONS_TEST_TOKEN", WRONG_TOKEN)
            .env("CARGO_REGISTRIES_DRAGONS_SECOND_TOKEN", WRONG_TOKEN)
            .args([
                format!("--token={SECOND_REGISTRY}:{LOGIN_TOKEN}"),
                "--token".to_owned(),
                format!("{REGISTRY}:{EXPLICIT_TOKEN}"),
            ])
            .assert()
            .success();
        assert_tokens_redacted(&assertion);
        if operation == "unleash" {
            assert_uploaded(&workspace, EXPLICIT_TOKEN, true);
            assert_registry_uploaded(&second, SECOND_PACKAGE, SECOND_VERSION, LOGIN_TOKEN, true);
        } else {
            assert_owner_request(&workspace.registry, PACKAGE, EXPLICIT_TOKEN);
            assert_owner_request(&second, SECOND_PACKAGE, LOGIN_TOKEN);
        }
        workspace.registry.assert_no_authorization(LOGIN_TOKEN);
        second.assert_no_authorization(EXPLICIT_TOKEN);
        workspace.registry.assert_no_authorization(WRONG_TOKEN);
        second.assert_no_authorization(WRONG_TOKEN);
        assert_eq!(credentials_snapshot(&workspace)?, credentials);
    }
    Ok(())
}

#[test]
fn unprovided_registry_uses_cargo_saved_or_environment_credentials() -> anyhow::Result<()> {
    for operation in ["unleash", "add-owner"] {
        for environment in [false, true] {
            let workspace = RegistryWorkspace::new()?;
            let second = select_second_registry(&workspace)?;
            if !environment {
                workspace.write_credentials(&format!(
                    "[registries.{SECOND_REGISTRY}]\ntoken = \"{LOGIN_TOKEN}\"\n"
                ))?;
            }
            let credentials = credentials_snapshot(&workspace)?;
            if operation == "add-owner" {
                workspace.registry.seed(PACKAGE, VERSION);
                second.seed(SECOND_PACKAGE, SECOND_VERSION);
            }
            let mut command = publication_command(&workspace, operation)?;
            command.args(["--token", &format!("{REGISTRY}:{EXPLICIT_TOKEN}")]);
            if environment {
                command.env("CARGO_REGISTRIES_DRAGONS_SECOND_TOKEN", LOGIN_TOKEN);
            }
            let assertion = command.assert().success();
            assert_tokens_redacted(&assertion);
            if operation == "unleash" {
                assert_uploaded(&workspace, EXPLICIT_TOKEN, true);
                assert_registry_uploaded(
                    &second,
                    SECOND_PACKAGE,
                    SECOND_VERSION,
                    LOGIN_TOKEN,
                    true,
                );
            } else {
                assert_owner_request(&workspace.registry, PACKAGE, EXPLICIT_TOKEN);
                assert_owner_request(&second, SECOND_PACKAGE, LOGIN_TOKEN);
            }
            workspace.registry.assert_no_authorization(LOGIN_TOKEN);
            second.assert_no_authorization(EXPLICIT_TOKEN);
            assert_eq!(credentials_snapshot(&workspace)?, credentials);
        }
    }
    Ok(())
}

#[test]
fn missing_any_unprovided_registry_login_fails_before_network_or_release_work() -> anyhow::Result<()>
{
    for operation in ["unleash", "unleash-dry-run", "add-owner"] {
        for (provided, missing) in [(REGISTRY, SECOND_REGISTRY), (SECOND_REGISTRY, REGISTRY)] {
            let workspace = RegistryWorkspace::new()?;
            let second = select_second_registry(&workspace)?;
            let credentials = credentials_snapshot(&workspace)?;
            let manifests = [PACKAGE, SECOND_PACKAGE]
                .map(|package| workspace.workspace.path().join(package).join("Cargo.toml"));
            let before = Result::<Vec<_>, _>::from_iter(manifests.iter().map(std::fs::read))?;

            let assertion = publication_command(&workspace, operation)?
                .args(["--token", &format!("{provided}:{EXPLICIT_TOKEN}")])
                .assert()
                .failure()
                .stderr(contains(missing))
                .stderr(contains(format!("cargo login --registry {missing}")));
            assert_tokens_redacted(&assertion);
            assert_no_requests(&workspace.registry);
            assert_no_requests(&second);
            for (path, contents) in manifests.iter().zip(before) {
                assert_eq!(std::fs::read(path)?, contents);
            }
            assert_eq!(credentials_snapshot(&workspace)?, credentials);
        }
    }
    Ok(())
}

#[test]
fn mixed_crates_io_selection_requires_login_for_each_unprovided_target() -> anyhow::Result<()> {
    for operation in ["unleash", "unleash-dry-run", "add-owner"] {
        for (provided, missing) in [("crates-io", REGISTRY), (REGISTRY, "crates-io")] {
            let workspace = RegistryWorkspace::new()?;
            remove_publish_setting(&workspace, SECOND_PACKAGE)?;
            let credentials = credentials_snapshot(&workspace)?;

            let assertion = publication_command(&workspace, operation)?
                .args(["--token", &format!("{provided}:{EXPLICIT_TOKEN}")])
                .assert()
                .failure()
                .stderr(contains(missing))
                .stderr(contains("cargo login"));
            assert_tokens_redacted(&assertion);
            assert_no_requests(&workspace.registry);
            assert_eq!(credentials_snapshot(&workspace)?, credentials);
        }
    }
    Ok(())
}

#[test]
fn token_prefix_must_name_a_selected_registry_not_just_a_configured_registry() -> anyhow::Result<()>
{
    for operation in ["unleash", "unleash-dry-run", "add-owner"] {
        for unselected in ["unknown-registry", SECOND_REGISTRY, "crates-io"] {
            let workspace = RegistryWorkspace::new()?;
            let second = select_second_registry(&workspace)?;
            workspace.write_credentials(&format!(
                "[registries.{REGISTRY}]\ntoken = \"{LOGIN_TOKEN}\"\n"
            ))?;
            let credentials = credentials_snapshot(&workspace)?;

            let assertion = publication_command(&workspace, operation)?
                .args([
                    "--packages",
                    PACKAGE,
                    "--token",
                    &format!("{unselected}:{EXPLICIT_TOKEN}"),
                ])
                .assert()
                .failure()
                .stderr(contains(unselected))
                .stderr(contains("selected"));
            assert_tokens_redacted(&assertion);
            assert_no_requests(&workspace.registry);
            assert_no_requests(&second);
            assert_eq!(credentials_snapshot(&workspace)?, credentials);
        }
    }
    Ok(())
}

#[test]
fn explicit_tokens_do_not_bypass_missing_selected_registry_configuration() -> anyhow::Result<()> {
    const UNCONFIGURED: &str = "unconfigured-publication-registry";
    for operation in ["unleash", "unleash-dry-run", "add-owner"] {
        for token in [
            EXPLICIT_TOKEN.to_owned(),
            format!("{UNCONFIGURED}:{EXPLICIT_TOKEN}"),
        ] {
            let workspace = RegistryWorkspace::new()?;
            let path = workspace.workspace.path().join(PACKAGE).join("Cargo.toml");
            let manifest = std::fs::read_to_string(&path)?;
            std::fs::write(path, manifest.replace(REGISTRY, UNCONFIGURED))?;
            let credentials = credentials_snapshot(&workspace)?;

            let assertion = publication_command(&workspace, operation)?
                .args(["--token", &token])
                .assert()
                .failure()
                .stderr(contains(UNCONFIGURED));
            assert_tokens_redacted(&assertion);
            assert_no_requests(&workspace.registry);
            assert_eq!(credentials_snapshot(&workspace)?, credentials);
        }
    }
    Ok(())
}

#[test]
fn duplicate_tokens_for_the_same_target_are_rejected_without_exposing_secrets() -> anyhow::Result<()>
{
    for operation in ["unleash", "unleash-dry-run", "add-owner"] {
        for tokens in [
            [EXPLICIT_TOKEN.to_owned(), LOGIN_TOKEN.to_owned()],
            [
                format!("{REGISTRY}:{EXPLICIT_TOKEN}"),
                format!("{REGISTRY}:{LOGIN_TOKEN}"),
            ],
            [
                EXPLICIT_TOKEN.to_owned(),
                format!("{REGISTRY}:{LOGIN_TOKEN}"),
            ],
            [
                format!("{REGISTRY}:{LOGIN_TOKEN}"),
                EXPLICIT_TOKEN.to_owned(),
            ],
            [
                format!("{REGISTRY}:{EXPLICIT_TOKEN}"),
                format!("{REGISTRY}:{EXPLICIT_TOKEN}"),
            ],
        ] {
            let workspace = RegistryWorkspace::new()?;
            workspace.write_credentials(&format!(
                "[registries.{REGISTRY}]\ntoken = \"{LOGIN_TOKEN}\"\n"
            ))?;
            let credentials = credentials_snapshot(&workspace)?;

            let assertion = publication_command(&workspace, operation)?
                .args(["--token", &tokens[0], "--token", &tokens[1]])
                .assert()
                .failure()
                .stderr(contains(REGISTRY))
                .stderr(
                    contains("duplicate")
                        .or(contains("Duplicate"))
                        .or(contains("more than once"))
                        .or(contains("multiple"))
                        .or(contains("Multiple")),
                );
            assert_tokens_redacted(&assertion);
            assert_no_requests(&workspace.registry);
            assert_eq!(credentials_snapshot(&workspace)?, credentials);
        }
    }
    Ok(())
}

#[test]
fn bare_and_prefixed_crates_io_tokens_are_duplicates_in_mixed_selection() -> anyhow::Result<()> {
    for operation in ["unleash", "unleash-dry-run", "add-owner"] {
        for bare_first in [false, true] {
            let workspace = RegistryWorkspace::new()?;
            remove_publish_setting(&workspace, SECOND_PACKAGE)?;
            workspace.write_credentials(&format!(
                "[registries.{REGISTRY}]\ntoken = \"{LOGIN_TOKEN}\"\n"
            ))?;
            let credentials = credentials_snapshot(&workspace)?;
            let mut tokens = [
                EXPLICIT_TOKEN.to_owned(),
                format!("crates-io:{LOGIN_TOKEN}"),
            ];
            if !bare_first {
                tokens.reverse();
            }

            let assertion = publication_command(&workspace, operation)?
                .args(["--token", &tokens[0], "--token", &tokens[1]])
                .assert()
                .failure()
                .stderr(contains("crates-io"))
                .stderr(
                    contains("duplicate")
                        .or(contains("Duplicate"))
                        .or(contains("more than once"))
                        .or(contains("multiple"))
                        .or(contains("Multiple")),
                );
            assert_tokens_redacted(&assertion);
            assert_no_requests(&workspace.registry);
            assert_eq!(credentials_snapshot(&workspace)?, credentials);
        }
    }
    Ok(())
}

#[test]
fn empty_registry_names_and_tokens_are_rejected_without_exposing_secrets() -> anyhow::Result<()> {
    for operation in ["unleash", "unleash-dry-run", "add-owner"] {
        for token in [
            String::new(),
            format!(":{EXPLICIT_TOKEN}"),
            format!("{REGISTRY}:"),
            ":".to_owned(),
        ] {
            let workspace = RegistryWorkspace::new()?;
            workspace.write_credentials(&format!(
                "[registries.{REGISTRY}]\ntoken = \"{LOGIN_TOKEN}\"\n"
            ))?;
            let credentials = credentials_snapshot(&workspace)?;

            let assertion = publication_command(&workspace, operation)?
                .args(["--token", &token])
                .assert()
                .failure()
                .stderr(contains("token").or(contains("Token")));
            assert_tokens_redacted(&assertion);
            assert_no_requests(&workspace.registry);
            assert_eq!(credentials_snapshot(&workspace)?, credentials);
        }
    }
    Ok(())
}
