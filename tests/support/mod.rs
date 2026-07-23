use assert_cmd::prelude::*;
use assert_fs::TempDir;
use fs_err as fs;
use std::{path::Path, path::PathBuf, process::Command};

pub struct TestWorkspace {
    temp: TempDir,
    cargo_home: TempDir,
}

impl TestWorkspace {
    pub fn from_fixture(name: &str) -> anyhow::Result<Self> {
        let temp = TempDir::new()?;
        let cargo_home = TempDir::new()?;
        let fixture = PathBuf::from("tests").join("fixtures").join(name);

        fs::metadata(&fixture)?;
        copy_dir_all(&fixture, temp.path())?;

        Ok(Self { temp, cargo_home })
    }

    pub fn path(&self) -> &Path {
        self.temp.path()
    }

    #[allow(dead_code)]
    pub fn read_to_string(&self, relative: impl AsRef<Path>) -> anyhow::Result<String> {
        Ok(fs::read_to_string(self.path().join(relative))?)
    }

    pub fn cargo_dragons(&self) -> anyhow::Result<Command> {
        let mut cmd = Command::cargo_bin("cargo-dragons")?;
        cmd.env("CARGO_HOME", self.cargo_home.path())
            .env("CARGO_NET_OFFLINE", "true")
            .env("CRATES_TOKEN", "cargo-dragons-dummy-token")
            .arg("--manifest-path")
            .arg(self.path());
        Ok(cmd)
    }

    #[allow(dead_code)]
    pub fn package_version(&self, manifest: impl AsRef<Path>) -> anyhow::Result<String> {
        let manifest = self.read_to_string(manifest)?;
        let manifest: toml::Value = toml::from_str(&manifest)?;
        let version = manifest
            .get("package")
            .and_then(|package| package.get("version"))
            .and_then(toml::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("manifest does not contain package.version"))?;
        Ok(version.to_owned())
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
