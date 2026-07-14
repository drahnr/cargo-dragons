mod support;

use assert_cmd::prelude::*;
use predicates::str::contains;
use support::TestWorkspace;

#[test]
fn check_include_pre() -> Result<(), Box<dyn std::error::Error>> {
	let ws = TestWorkspace::from_fixture("include-pre")?;

	let mut cmd = ws.cargo_dragons()?;
	cmd.arg("to-release").arg("--packages").arg("crate_a").arg("--include-pre-deps");

	cmd.assert()
		.success()
		.code(0)
		.stdout(contains("unicode-width (10.0.0-dev)"))
		.stdout(contains("cu-left-pad (1.0.0-dev)"));
	Ok(())
}
