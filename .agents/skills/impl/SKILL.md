---
name: impl
description: Implement the requested change in this Rust project, using trait-based production/mock abstractions and validating with workspace clippy. Use only when explicitly invoked as /impl.
disable-model-invocation: true
---

# Implementation Workflow

Implement the user's requested change completely and with minimal, focused edits. Inspect the relevant code and existing patterns before making changes. Update tests, call sites, and documentation when they are directly affected.

## Required abstraction rule

For behavior that differs between production and tests/mocks, define a focused trait and provide separate concrete implementations. Express callers' requirements with trait bounds or trait objects only where dynamic dispatch is actually needed. Do not model production-versus-mock selection with an enum, and do not introduce enums as wrappers or dispatch switches for those implementations.

Prefer small interfaces, dependency injection, and existing project conventions. Avoid unrelated refactors and new dependencies unless justified.

## Required validation

After implementation:

1. Run focused tests for the changed behavior when available.
2. Run `cargo clippy --workspace` and resolve issues caused by the implementation without blanket lint allowances.
3. Report the files changed, behavior delivered, and exact validation results.

If blocked by missing requirements or an unresolvable failure, explain the concrete blocker and what remains.