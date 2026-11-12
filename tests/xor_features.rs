mod support;

use assert_cmd::prelude::*;
use assert_fs::TempDir;
use predicates::str::contains;
use std::process::Command;
use support::TestWorkspace;

#[test]
fn xor_features_require_exactly_one_backend() -> Result<(), Box<dyn std::error::Error>> {
    let ws = TestWorkspace::from_fixture("xor-features")?;
    let cargo_home = TempDir::new()?;

    for (features, expected_error) in [
        (
            "",
            Some("exactly one backend is required: enable backend-a or backend-b"),
        ),
        ("backend-a", None),
        ("backend-b", None),
        (
            "backend-a,backend-b",
            Some("backend-a and backend-b are mutually exclusive"),
        ),
    ] {
        let mut cmd = Command::new(env!("CARGO"));
        cmd.current_dir(ws.path())
            .env("CARGO_HOME", cargo_home.path())
            .arg("check")
            .arg("--offline")
            .arg("--no-default-features");
        if !features.is_empty() {
            cmd.arg("--features").arg(features);
        }

        match expected_error {
            Some(message) => {
                cmd.assert().failure().stderr(contains(message));
            }
            None => {
                cmd.assert().success();
            }
        }
    }

    Ok(())
}

#[test]
fn independence_check_currently_rejects_xor_features() -> Result<(), Box<dyn std::error::Error>> {
    for context in ["inplace", "ephemeral"] {
        let ws = TestWorkspace::from_fixture("xor-features")?;
        let mut cmd = ws.cargo_dragons()?;
        cmd.arg("independence-check")
            .arg("--mode")
            .arg("check")
            .arg("--ctx")
            .arg(context);

        // This fixture has no default backend, so the empty feature set is invalid.
        cmd.assert()
            .failure()
            .stderr(contains("xor-features 0.1.0 with features []"))
            .stderr(contains(
                "exactly one backend is required: enable backend-a or backend-b",
            ));
    }

    Ok(())
}
