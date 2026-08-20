mod support;

use assert_cmd::prelude::*;
use fs_err as fs;
use predicates::{prelude::PredicateBooleanExt, str::contains};
use support::TestWorkspace;

#[test]
fn check_include_pre() -> Result<(), Box<dyn std::error::Error>> {
    let ws = TestWorkspace::from_fixture("include-pre")?;

    let mut cmd = ws.cargo_dragons()?;
    cmd.arg("to-release")
        .arg("--packages")
        .arg("crate-a")
        .arg("--include-pre-deps");

    cmd.assert()
        .success()
        .code(0)
        .stdout(contains("crate-a (0.1.0)"))
        .stdout(contains("unicode-width"))
        .stdout(contains("cu-left-pad (1.0.0-dev)"));
    Ok(())
}

#[test]
fn to_release_selects_all_by_default() -> Result<(), Box<dyn std::error::Error>> {
    let ws = TestWorkspace::from_fixture("simple-base")?;

    let mut cmd = ws.cargo_dragons()?;
    cmd.arg("to-release");

    cmd.assert()
        .success()
        .code(0)
        .stdout(contains("crateA (0.1.0)"))
        .stdout(contains("crateB (2.0.0)"))
        .stdout(contains("crateC (3.1.0)"));
    Ok(())
}

#[test]
fn to_release_does_not_select_dev_only_path_dependencies() -> Result<(), Box<dyn std::error::Error>>
{
    let ws = TestWorkspace::from_fixture("dev-only-path-dep")?;

    let mut cmd = ws.cargo_dragons()?;
    cmd.arg("to-release");

    cmd.assert()
        .success()
        .code(0)
        .stdout(contains("app (0.1.0)"))
        .stdout(contains("dev-helper").not());
    Ok(())
}

#[test]
fn to_release_handles_manifests_with_dev_dependency_back_edges()
-> Result<(), Box<dyn std::error::Error>> {
    let ws = TestWorkspace::from_fixture("dev-dep-cycle")?;

    let mut cmd = ws.cargo_dragons()?;
    cmd.arg("to-release");

    cmd.assert()
        .success()
        .code(0)
        .stdout(contains("cycle-helper (0.1.0), cycle-consumer (0.1.0)"));
    Ok(())
}

#[test]
fn to_release_does_not_remove_dev_dependencies() -> Result<(), Box<dyn std::error::Error>> {
    let ws = TestWorkspace::from_fixture("dev-dep-cycle")?;

    let mut cmd = ws.cargo_dragons()?;
    cmd.arg("to-release");

    cmd.assert()
        .success()
        .code(0)
        .stdout(contains("cycle-helper (0.1.0), cycle-consumer (0.1.0)"));

    let manifest = fs::read_to_string(ws.path().join("cycle-helper/Cargo.toml"))?;
    assert!(
        manifest.contains("[dev-dependencies]"),
        "to-release must not edit canonical manifests"
    );
    Ok(())
}

#[test]
fn check_ignores_dev_dependency_back_edges() -> Result<(), Box<dyn std::error::Error>> {
    let ws = TestWorkspace::from_fixture("dev-dep-cycle")?;

    let mut cmd = ws.cargo_dragons()?;
    cmd.arg("check");

    cmd.assert().success().code(0);

    let manifest = fs::read_to_string(ws.path().join("cycle-helper/Cargo.toml"))?;
    assert!(
        manifest.contains("[dev-dependencies]"),
        "check must not edit canonical manifests"
    );
    Ok(())
}

fn add_publish_false(ws: &TestWorkspace, relative: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = ws.path().join(relative);
    let manifest = fs::read_to_string(&path)?;
    fs::write(
        path,
        manifest.replace("edition = \"2018\"", "edition = \"2018\"\npublish = false"),
    )?;
    Ok(())
}

#[test]
fn no_package_selector_lists_available_packages_when_release_set_is_empty()
-> Result<(), Box<dyn std::error::Error>> {
    let ws = TestWorkspace::from_fixture("simple-base")?;
    add_publish_false(&ws, "crateA/Cargo.toml")?;
    add_publish_false(&ws, "crateB/Cargo.toml")?;
    add_publish_false(&ws, "crateC/Cargo.toml")?;

    let mut cmd = ws.cargo_dragons()?;
    cmd.arg("to-release");

    cmd.assert()
        .success()
        .stdout(contains("No packages selected"))
        .stdout(contains("Available packages"))
        .stdout(contains("\u{1b}[1mcrateA"))
        .stdout(contains("crateB"))
        .stdout(contains("crateC"))
        .stdout(contains("How to fix:"))
        .stdout(contains("bump the version"))
        .stdout(contains("--packages <name>"));
    Ok(())
}

#[test]
fn list_packages_lists_available_packages() -> Result<(), Box<dyn std::error::Error>> {
    let ws = TestWorkspace::from_fixture("include-pre")?;

    let mut cmd = ws.cargo_dragons()?;
    cmd.arg("to-release").arg("--list-packages");

    cmd.assert()
        .success()
        .stdout(contains("crate-a"))
        .stdout(contains("cu-left-pad"))
        .stdout(contains("unicode-width"))
        .stderr(contains("No package selector provided").not());
    Ok(())
}

#[test]
fn comma_separated_packages_are_selected() -> Result<(), Box<dyn std::error::Error>> {
    let ws = TestWorkspace::from_fixture("simple-base")?;

    let mut cmd = ws.cargo_dragons()?;
    cmd.arg("to-release").arg("-p").arg("crateA,crateC");

    cmd.assert()
        .success()
        .stdout(contains("crateA (0.1.0)"))
        .stdout(contains("crateC (3.1.0)"))
        .stdout(contains("crateB").not());
    Ok(())
}

#[test]
fn missing_package_selector_lists_available_packages_and_suggestion()
-> Result<(), Box<dyn std::error::Error>> {
    let ws = TestWorkspace::from_fixture("include-pre")?;

    let mut cmd = ws.cargo_dragons()?;
    cmd.arg("to-release").arg("--packages").arg("crate_a");

    cmd.assert()
        .failure()
        .stderr(contains(
            "Package selector `crate_a` did not match any packages",
        ))
        .stderr(contains("Available packages"))
        .stderr(contains("\u{1b}[1mcrate-a"))
        .stderr(contains("crate-a"))
        .stderr(contains("cu-left-pad"))
        .stderr(contains("Did you mean `"))
        .stderr(contains("crate-a"))
        .stderr(contains("How to fix:"))
        .stderr(contains("--packages crate-a"));
    Ok(())
}
