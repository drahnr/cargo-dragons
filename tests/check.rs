use assert_cmd::prelude::*;
use assert_fs::prelude::*;
use predicates::str::contains;
use std::process::Command;

#[test]
fn check_include_pre() -> Result<(), Box<dyn std::error::Error>> {
	let temp = assert_fs::TempDir::new()?;
	temp.copy_from("tests/fixtures/include-pre", &["*.toml", "*.rs"])?;

	let mut cmd = Command::cargo_bin("cargo-dragons")?;

	cmd.env("CARGO_NET_OFFLINE", "true")
		.arg("--manifest-path")
		.arg(temp.path())
		.arg("to-release")
		.arg("--packages")
		.arg("crate_a")
		.arg("--include-pre-deps");

	cmd.assert()
		.success()
		.code(0)
		.stdout(contains("unicode-width (10.0.0-dev)"))
		.stdout(contains("cu-left-pad (1.0.0-dev)"));
	temp.close()?;
	Ok(())
}
