---
name: review
description: Review the current change for correctness, maintainability, tests, and Rust quality. Use only when explicitly invoked as /review.
disable-model-invocation: true
---

# Change Review

Review the current working-tree changes as a code reviewer. First inspect the diff and relevant surrounding code, then report findings ordered by severity.

Check for:

- Correctness, error handling, edge cases, and regressions.
- Public API and behavioral compatibility.
- Test coverage that meaningfully protects changed behavior.
- Idiomatic, maintainable Rust and consistency with existing project patterns.
- Test/production or test/mock abstractions modeled with traits and implementations, not enums used to select implementations.

Run focused validation where practical. State the commands run and their outcomes. Do not modify files unless the user asks you to address the review findings.

If no actionable findings exist, say so explicitly and identify any remaining validation gaps.