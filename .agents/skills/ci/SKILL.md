---
name: ci
description: Run the repository's local Rust CI-quality checks and report actionable failures without making unrelated changes. Use only when explicitly invoked as /ci.
disable-model-invocation: true
---

# Local CI Verification

First inspect repository automation and manifests for project-specific checks. If no CI configuration adds requirements, run this baseline from the repository root:

```sh
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

Run independent commands separately so each outcome is reported accurately. Use a bounded timeout for potentially long commands.

If a check fails:

- Report the exact command and relevant failure output.
- Fix failures caused by the current requested work when their root cause is clear.
- Do not change code merely to hide unrelated or pre-existing failures.
- Re-run every check invalidated by a fix.

Do not create commits, tags, releases, or publish packages. Finish with a pass/fail summary for every check and identify environmental blockers separately from code failures.