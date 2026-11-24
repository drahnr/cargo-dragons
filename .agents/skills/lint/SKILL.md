---
name: lint
description: Validate Rust code quality and abstraction boundaries, including trait-based production/mock seams, then run cargo clippy --workspace. Use only when explicitly invoked as /lint.
disable-model-invocation: true
---

# Rust Lint and Abstraction Check

Inspect the current code and working-tree changes for lint and design violations, then fix issues that are clearly within scope.

## Required abstraction rule

Where code needs interchangeable production and test/mock behavior, use trait bounds with concrete implementations. Do not use enums as switches or wrappers for production-versus-mock implementations. Keep dependencies injectable through trait-based interfaces and preserve static dispatch where it fits the existing design.

Do not introduce an enum solely to choose, carry, or branch between test and production implementations. Existing violations should be called out; only refactor them when it is necessary for the requested work or the user asks.

## Validation

1. Run `cargo clippy --workspace`.
2. Resolve clippy failures caused by the current change, without suppressing lints unnecessarily.
3. Report the command result and any unrelated pre-existing failures.

Also run focused tests when they are relevant and affordable. Summarize changed files and validation outcomes.