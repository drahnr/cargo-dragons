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

    pub fn cargo_dragons(&self) -> anyhow::Result<Command> {
        let mut cmd = Command::cargo_bin("cargo-dragons")?;
        cmd.env("CARGO_HOME", self.cargo_home.path())
            .env("CARGO_NET_OFFLINE", "true")
            .env("CARGO_REGISTRY_TOKEN", "cargo-dragons-dummy-token")
            .arg("--manifest-path")
            .arg(self.path());
        Ok(cmd)
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
