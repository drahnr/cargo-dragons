---
name: development-cycle
description: Run a complete Rust development cycle architecture assessment, implementation, testing, linting, and review using /arch, /impl, /test, /lint, and /review standards. Use only when explicitly invoked as /development-cycle.
disable-model-invocation: true
---

# Development Cycle

Take the user's requested change from assessment through validated, review-ready completion. Follow this cycle in order and apply the corresponding project skill standards at every stage.

## 1. Architecture — `/arch`

Before editing, inspect the relevant code, module boundaries, public APIs, dependency direction, and existing tests. Identify only architecture concerns material to the requested work.

Ensure any production/test or production/mock variation is designed around focused traits with concrete implementations. Do not propose or introduce enums that select or wrap those implementations.

If the request is small and the existing architecture clearly accommodates it, keep this phase brief and proceed.

## 2. Implementation — `/impl`

Implement the requested behavior with minimal, focused changes. Follow local conventions, update directly affected tests and call sites, and avoid unrelated refactors.

Use trait bounds and concrete implementations for production/mock seams. Do not use enums to select, contain, or branch between production and test/mock alternatives.

## 3. Tests and coverage — `/test`

Add or update meaningful behavior-focused tests for the changed behavior. Use trait-based mocks and reusable harness helpers where they improve clarity. Avoid testing trivialities such as plain `::new` constructors, simple accessors, or field assignment.

Run focused tests first. When available, generate LCOV coverage with:

```sh
cargo llvm-cov --workspace --lcov --output-path lcov.info
```

Use coverage to find meaningful untested behavior, not to justify low-value tests.

## 4. Lint and validation — `/lint`

Inspect the completed change for the required trait-based abstraction rule, then run:

```sh
cargo clippy --workspace
```

Fix failures caused by the change without blanket lint suppressions. Clearly distinguish unrelated or pre-existing failures from the change's results.

## 5. Final review — `/review`

Review the final diff and relevant context for correctness, edge cases, error handling, compatibility, test coverage, maintainability, and the trait-based abstraction rule. Address actionable issues found during the review when they are within scope, then rerun any validation invalidated by the fixes.

## Completion report

Finish with a concise report that includes:

- Architecture decisions or noteworthy tradeoffs.
- Files changed and behavior delivered.
- Test, LCOV coverage, and `cargo clippy --workspace` commands run with their outcomes.
- Any remaining risks, known limitations, or validation gaps.

Do not stop after planning: carry the work through implementation and validation unless a concrete requirement or environment blocker prevents it.
