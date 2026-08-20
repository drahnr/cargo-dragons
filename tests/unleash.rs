mod support;

use assert_cmd::prelude::*;
use assert_fs::TempDir;
use fs_err as fs;
use predicates::{prelude::PredicateBooleanExt, str::contains};
use std::{path::Path, process::Command};
use support::TestWorkspace;
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

fn replace_in_fixture(
    ws: &TestWorkspace,
    relative: &str,
    from: &str,
    to: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = ws.path().join(relative);
    let contents = fs::read_to_string(&path)?;
    fs::write(path, contents.replace(from, to))?;
    Ok(())
}

fn add_publish_false(ws: &TestWorkspace, relative: &str) -> Result<(), Box<dyn std::error::Error>> {
    replace_in_fixture(
        ws,
        relative,
        "edition = \"2018\"",
        "edition = \"2018\"\npublish = false",
    )
}

#[derive(Clone, Copy)]
struct GraphPackage {
    name: &'static str,
    version: &'static str,
    dependencies: &'static [(&'static str, &'static str)],
}

fn toml_string(value: impl Into<String>) -> Item {
    Item::Value(Value::from(value.into()).decorated(" ", ""))
}

fn graph_workspace_manifest(packages: &[GraphPackage]) -> DocumentMut {
    let mut document = DocumentMut::new();
    let mut workspace = Table::new();
    let mut members = Array::new();
    for package in packages {
        members.push(package.name);
    }
    workspace["resolver"] = toml_string("2");
    workspace["members"] = Item::Value(Value::Array(members));
    document["workspace"] = Item::Table(workspace);
    document
}

fn graph_package_manifest(package: &GraphPackage) -> DocumentMut {
    let mut document = DocumentMut::new();

    let mut package_table = Table::new();
    package_table["name"] = toml_string(package.name);
    package_table["version"] = toml_string(package.version);
    package_table["edition"] = toml_string("2021");
    package_table["license"] = toml_string("MIT");
    package_table["description"] = toml_string(format!(
        "generated release graph fixture for {}",
        package.name
    ));
    package_table["repository"] = toml_string("https://example.invalid/cargo-dragons");
    document["package"] = Item::Table(package_table);

    let mut dependencies = Table::new();
    for (name, version) in package.dependencies {
        let mut dependency = InlineTable::new();
        dependency.insert("version", Value::from(*version));
        dependency.insert("path", Value::from(format!("../{name}")));
        dependency.fmt();
        dependencies[*name] = Item::Value(Value::InlineTable(dependency));
    }
    document["dependencies"] = Item::Table(dependencies);

    document
}

fn write_graph_workspace(
    root: &Path,
    packages: &[GraphPackage],
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(root)?;
    fs::write(
        root.join("Cargo.toml"),
        graph_workspace_manifest(packages).to_string(),
    )?;

    for package in packages {
        let package_dir = root.join(package.name);
        fs::create_dir_all(package_dir.join("src"))?;
        fs::write(
            package_dir.join("Cargo.toml"),
            graph_package_manifest(package).to_string(),
        )?;
        fs::write(
            package_dir.join("src/lib.rs"),
            format!("pub fn {}() {{}}\n", package.name.replace('-', "_")),
        )?;
    }

    Ok(())
}

fn generated_workspace_cargo_dragons(
    manifest_path: &Path,
    cargo_home: &Path,
) -> Result<Command, Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("cargo-dragons")?;
    cmd.env("CARGO_HOME", cargo_home)
        .env("CARGO_NET_OFFLINE", "true")
        .env("CARGO_REGISTRY_TOKEN", "cargo-dragons-dummy-token")
        .arg("--manifest-path")
        .arg(manifest_path);
    Ok(cmd)
}

fn assert_dry_run_unleash_packages_generated_graph(
    packages: &[GraphPackage],
) -> Result<(), Box<dyn std::error::Error>> {
    let workspace = TempDir::new()?;
    let cargo_home = TempDir::new()?;
    write_graph_workspace(workspace.path(), packages)?;

    let mut cmd = generated_workspace_cargo_dragons(workspace.path(), cargo_home.path())?;
    cmd.arg("unleash").arg("--dry-run").arg("--skip-verify");

    let assert = cmd.assert().success();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("Dry-run packaging"),
        "expected dry-run packaging, stderr:\n{stderr}"
    );
    assert_eq!(
        packages.len(),
        stderr.matches("Would package").count(),
        "expected every package to be listed exactly once, stderr:\n{stderr}"
    );
    for package in packages {
        assert!(
            stderr.contains(package.name),
            "expected output to mention {}, stderr:\n{stderr}",
            package.name
        );
    }

    Ok(())
}

fn assert_dry_run_unleash_rejects_generated_cycle(
    packages: &[GraphPackage],
) -> Result<(), Box<dyn std::error::Error>> {
    let workspace = TempDir::new()?;
    let cargo_home = TempDir::new()?;
    write_graph_workspace(workspace.path(), packages)?;

    let mut cmd = generated_workspace_cargo_dragons(workspace.path(), cargo_home.path())?;
    cmd.arg("unleash").arg("--dry-run").arg("--skip-verify");

    cmd.assert()
        .failure()
        .stderr(contains("Cycles"))
        .stderr(contains("Dry-run packaging").not());
    Ok(())
}

#[test]
fn dry_run_unleash_handles_diamond_release_graph() -> Result<(), Box<dyn std::error::Error>> {
    assert_dry_run_unleash_packages_generated_graph(&[
        GraphPackage {
            name: "top",
            version: "0.1.2",
            dependencies: &[("dx", "1.11"), ("dy", "15")],
        },
        GraphPackage {
            name: "dx",
            version: "1.11.111",
            dependencies: &[("closing", "1.6.4")],
        },
        GraphPackage {
            name: "dy",
            version: "15.100.0",
            dependencies: &[("closing", "1.6.1")],
        },
        GraphPackage {
            name: "closing",
            version: "1.6.9",
            dependencies: &[],
        },
    ])
}

#[test]
fn dry_run_unleash_rejects_circular_release_graph() -> Result<(), Box<dyn std::error::Error>> {
    assert_dry_run_unleash_rejects_generated_cycle(&[
        GraphPackage {
            name: "a",
            version: "3.0.0",
            dependencies: &[("b", "*")],
        },
        GraphPackage {
            name: "b",
            version: "2.0.0",
            dependencies: &[("c", "*")],
        },
        GraphPackage {
            name: "c",
            version: "1.0.0",
            dependencies: &[("a", "*")],
        },
    ])
}

#[test]
fn dry_run_unleash_handles_larger_diamond_release_graph() -> Result<(), Box<dyn std::error::Error>>
{
    assert_dry_run_unleash_packages_generated_graph(&[
        GraphPackage {
            name: "app",
            version: "1.0.0",
            dependencies: &[("left", "1"), ("middle", "1"), ("right", "1")],
        },
        GraphPackage {
            name: "left",
            version: "1.1.0",
            dependencies: &[("shared-a", "1"), ("shared-b", "1")],
        },
        GraphPackage {
            name: "middle",
            version: "1.2.0",
            dependencies: &[("shared-b", "1")],
        },
        GraphPackage {
            name: "right",
            version: "1.3.0",
            dependencies: &[("shared-a", "1"), ("shared-b", "1")],
        },
        GraphPackage {
            name: "shared-a",
            version: "1.4.0",
            dependencies: &[("foundation", "1")],
        },
        GraphPackage {
            name: "shared-b",
            version: "1.5.0",
            dependencies: &[("foundation", "1")],
        },
        GraphPackage {
            name: "foundation",
            version: "1.6.0",
            dependencies: &[],
        },
    ])
}

#[test]
fn dry_run_unleash_rejects_larger_circular_release_graph() -> Result<(), Box<dyn std::error::Error>>
{
    assert_dry_run_unleash_rejects_generated_cycle(&[
        GraphPackage {
            name: "a",
            version: "1.0.0",
            dependencies: &[("b", "*")],
        },
        GraphPackage {
            name: "b",
            version: "1.0.0",
            dependencies: &[("c", "*")],
        },
        GraphPackage {
            name: "c",
            version: "1.0.0",
            dependencies: &[("d", "*")],
        },
        GraphPackage {
            name: "d",
            version: "1.0.0",
            dependencies: &[("e", "*")],
        },
        GraphPackage {
            name: "e",
            version: "1.0.0",
            dependencies: &[("f", "*")],
        },
        GraphPackage {
            name: "f",
            version: "1.0.0",
            dependencies: &[("a", "*")],
        },
        GraphPackage {
            name: "outside",
            version: "1.0.0",
            dependencies: &[("a", "*")],
        },
    ])
}

#[test]
fn dry_run_unleash_requires_login() -> Result<(), Box<dyn std::error::Error>> {
    let ws = TestWorkspace::from_fixture("include-pre")?;

    let mut cmd = ws.cargo_dragons()?;
    // cargo-dragons intentionally uses Cargo's native crates.io token env var
    // instead of a custom alias. Clear it so this missing-login test remains
    // hermetic even on machines that export Cargo credentials.
    cmd.env_remove("CARGO_REGISTRY_TOKEN")
        .arg("unleash")
        .arg("--dry-run")
        .arg("--skip-verify");

    cmd.assert()
        .failure()
        .stderr(contains("Not logged in to crates.io"))
        .stderr(contains("cargo login"))
        .stderr(contains("CARGO_REGISTRY_TOKEN"));
    Ok(())
}

#[test]
fn dry_run_unleash_accepts_token_without_rewriting_credentials()
-> Result<(), Box<dyn std::error::Error>> {
    let ws = TestWorkspace::from_fixture("include-pre")?;
    let cargo_home = TempDir::new()?;
    let credentials = cargo_home.path().join("credentials.toml");
    let existing_credentials = "[registries.other]\ntoken = \"keep-me\"\n";
    fs::write(&credentials, existing_credentials)?;

    let mut cmd = ws.cargo_dragons()?;
    cmd.env("CARGO_HOME", cargo_home.path())
        .env_remove("CARGO_REGISTRY_TOKEN")
        .arg("unleash")
        .arg("--token")
        .arg("cargo-dragons-dummy-token")
        .arg("--dry-run")
        .arg("--skip-verify");

    cmd.assert()
        .success()
        .stderr(contains("Dry-run packaging"))
        .stderr(contains("Would package"));
    assert_eq!(fs::read_to_string(credentials)?, existing_credentials);
    Ok(())
}

#[test]
fn dry_run_unleash_lists_available_packages_and_exits_when_release_set_is_empty()
-> Result<(), Box<dyn std::error::Error>> {
    let ws = TestWorkspace::from_fixture("simple-base")?;
    add_publish_false(&ws, "crateA/Cargo.toml")?;
    add_publish_false(&ws, "crateB/Cargo.toml")?;
    add_publish_false(&ws, "crateC/Cargo.toml")?;

    let mut cmd = ws.cargo_dragons()?;
    cmd.arg("unleash").arg("--dry-run").arg("--skip-verify");

    cmd.assert()
        .success()
        .stdout(contains("No packages selected"))
        .stdout(contains("Available packages"))
        .stdout(contains("\u{1b}[1mcrateA"))
        .stdout(contains("How to fix:"))
        .stdout(contains("--packages <name>"))
        .stderr(contains("Releasing").not())
        .stderr(contains("Dry-run packaging").not())
        .stderr(contains("Would package").not());
    Ok(())
}

#[test]
fn dry_run_unleash_packages_workspace_dependencies() -> Result<(), Box<dyn std::error::Error>> {
    let ws = TestWorkspace::from_fixture("include-pre")?;

    let mut cmd = ws.cargo_dragons()?;
    cmd.arg("unleash")
        .arg("--packages")
        .arg("cu-left-pad")
        .arg("--include-pre-deps")
        .arg("--dry-run")
        .arg("--skip-verify");

    cmd.assert()
        .success()
        .stderr(contains("Dry-run packaging"))
        .stderr(contains("unicode-width"))
        .stderr(contains("cu-left-pad"));
    Ok(())
}

#[test]
fn dry_run_unleash_packages_self_by_default() -> Result<(), Box<dyn std::error::Error>> {
    let ws = TestWorkspace::from_fixture("include-pre")?;

    let mut cmd = ws.cargo_dragons()?;
    cmd.arg("unleash")
        .arg("--packages")
        .arg("crate-a")
        .arg("--include-pre-deps")
        .arg("--dry-run")
        .arg("--skip-verify");

    cmd.assert()
        .success()
        .stderr(contains("Dry-run packaging"))
        .stderr(contains("Would package"))
        .stderr(contains("crate-a v"))
        .stderr(contains("cu-left-pad"));
    Ok(())
}

#[test]
fn dry_run_unleash_without_self_packages_only_dependencies()
-> Result<(), Box<dyn std::error::Error>> {
    let ws = TestWorkspace::from_fixture("include-pre")?;

    let mut cmd = ws.cargo_dragons()?;
    cmd.arg("unleash")
        .arg("--packages")
        .arg("crate-a")
        .arg("--include-pre-deps")
        .arg("--without-self")
        .arg("--dry-run")
        .arg("--skip-verify");

    cmd.assert()
        .success()
        .stderr(contains("Dry-run packaging"))
        .stderr(contains("Would package"))
        .stderr(contains("cu-left-pad"))
        .stderr(contains("crate-a v").not())
        .stderr(contains("Checking Packages").not());
    Ok(())
}

#[test]
fn dry_run_unleash_allows_unpublished_workspace_pre_release_dependencies()
-> Result<(), Box<dyn std::error::Error>> {
    let ws = TestWorkspace::from_fixture("include-pre")?;
    replace_in_fixture(&ws, "unicode-width/Cargo.toml", "0.2.2-dev", "10.0.0-dev")?;
    replace_in_fixture(&ws, "cu-left-pad/Cargo.toml", "0.2.2-dev", "10.0.0-dev")?;

    let mut cmd = ws.cargo_dragons()?;
    cmd.arg("-v")
        .arg("unleash")
        .arg("--packages")
        .arg("cu-left-pad")
        .arg("--include-pre-deps")
        .arg("--dry-run");

    cmd.assert()
        .success()
        .stderr(contains("Checking Packages"))
        .stderr(contains("Dry-run packaging"))
        .stderr(contains("Would package"))
        .stderr(contains("unicode-width"))
        .stderr(contains("10.0.0-dev"))
        .stderr(contains("cu-left-pad"));
    Ok(())
}
