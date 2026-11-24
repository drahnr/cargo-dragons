---
name: debug
description: Diagnose and fix a Rust defect by reproducing it, identifying the root cause, adding a regression test, and validating the focused fix. Use only when explicitly invoked as /debug.
disable-model-invocation: true
---

# Debugging Workflow

Investigate the reported problem to a confirmed root cause before modifying code.

1. Capture the expected and actual behavior, relevant inputs, environment, command, and complete error output.
2. Reproduce the issue with the smallest reliable command or test. If it cannot be reproduced, inspect the failing path and state the missing evidence rather than guessing.
3. Trace the relevant code and isolate the root cause. Distinguish evidence from hypotheses.
4. Add or update a focused regression test that fails before the fix and passes afterward. Test observable behavior; do not test trivial constructors or internal implementation details.
5. Implement the smallest correct fix. For test/prod variations, use focused traits and concrete mock implementations, never enums that select or wrap implementations.
6. Run the regression test, related tests, `cargo clippy --workspace`, and any broader validation warranted by the change.

Do not paper over a defect with broad error suppression, a retry without a cause, an unrelated refactor, or a blanket lint allowance. Report the reproduction, root cause, files changed, regression coverage, commands run, and any remaining uncertainty.