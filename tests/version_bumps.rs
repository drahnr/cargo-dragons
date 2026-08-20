mod support;

use assert_cmd::prelude::*;
use fs_err as fs;
use std::path::Path;
use support::TestWorkspace;

fn package_version(ws: &TestWorkspace, manifest: impl AsRef<Path>) -> anyhow::Result<String> {
    let manifest = fs::read_to_string(ws.path().join(manifest))?;
    let manifest: toml::Value = toml::from_str(&manifest)?;
    let version = manifest
        .get("package")
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("manifest does not contain package.version"))?;
    Ok(version.to_owned())
}

#[test]
fn set_pre() -> Result<(), Box<dyn std::error::Error>> {
    let ws = TestWorkspace::from_fixture("simple-base")?;

    let mut cmd = ws.cargo_dragons()?;
    cmd.arg("version")
        .arg("set-pre")
        .arg("dev")
        .arg("--packages")
        .arg("crate(A|B)");
    cmd.assert().success();

    assert_eq!(package_version(&ws, "crateA/Cargo.toml")?, "0.1.0-dev");
    assert_eq!(package_version(&ws, "crateB/Cargo.toml")?, "2.0.0-dev");
    assert_eq!(package_version(&ws, "crateC/Cargo.toml")?, "3.1.0"); // wasn't selected
    Ok(())
}

#[test]
fn bump_to_dev() -> Result<(), Box<dyn std::error::Error>> {
    let ws = TestWorkspace::from_fixture("simple-base")?;

    let mut cmd = ws.cargo_dragons()?;
    cmd.arg("version")
        .arg("bump-to-dev")
        .arg("--packages")
        .arg("crate.*");
    cmd.assert().success();

    assert_eq!(package_version(&ws, "crateA/Cargo.toml")?, "0.2.0-dev");
    assert_eq!(package_version(&ws, "crateB/Cargo.toml")?, "3.0.0-dev");
    assert_eq!(package_version(&ws, "crateC/Cargo.toml")?, "4.0.0-dev");
    Ok(())
}
