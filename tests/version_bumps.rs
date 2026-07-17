mod support;

use assert_cmd::prelude::*;
use support::TestWorkspace;

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

    assert_eq!(ws.package_version("crateA/Cargo.toml")?, "0.1.0-dev");
    assert_eq!(ws.package_version("crateB/Cargo.toml")?, "2.0.0-dev");
    assert_eq!(ws.package_version("crateC/Cargo.toml")?, "3.1.0"); // wasn't selected
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

    assert_eq!(ws.package_version("crateA/Cargo.toml")?, "0.2.0-dev");
    assert_eq!(ws.package_version("crateB/Cargo.toml")?, "3.0.0-dev");
    assert_eq!(ws.package_version("crateC/Cargo.toml")?, "4.0.0-dev");
    Ok(())
}
