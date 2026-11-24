---
name: test
description: Design, implement, and validate meaningful Rust tests with trait-based mocks, reusable harnesses, and LCOV coverage reporting. Use only when explicitly invoked as /test.
disable-model-invocation: true
---

# Rust Testing and Coverage

Add or improve tests for the user's requested behavior. First inspect the production code, existing tests, and test helpers so tests fit the project’s established patterns.

## Test design

- Test observable behavior, important error paths, boundary conditions, and meaningful integration points.
- Do not add tests for trivialities, including plain `::new` constructors, simple field assignment/access, or other code with no branching, invariants, transformations, or externally observable behavior worth protecting.
- Use descriptive test names that state the scenario and expected result.
- Keep tests deterministic, isolated, and independent of machine-specific state, network access, clock time, and test ordering.
- Prefer idiomatic Rust assertions and existing test dependencies and helpers.

## Mocks and test harnesses

- Create test seams with focused traits and use concrete test/mock implementations. Do not use enums to select, store, or dispatch between production and mock implementations.
- Put dependency boundaries at I/O, external services, filesystem/process interaction, and expensive or nondeterministic work—not at incidental implementation details.
- Generalize test harness and fixture code early when two or more tests need the same setup, builders, temporary-workspace scaffolding, command invocation, or assertions. Keep shared helpers small, composable, and close to the tests that use them.
- Do not over-generalize a helper used only once; retain readable local setup when it is clearer.

## Validation and LCOV coverage

1. Run the focused test target(s) first, then `cargo test --workspace` when practical.
2. Generate LCOV coverage with `cargo llvm-cov` when it is available:

   ```sh
   cargo llvm-cov --workspace --lcov --output-path lcov.info
   ```

3. Inspect coverage for the changed behavior and add tests for meaningful uncovered paths. Coverage percentage is a signal, not a goal: do not add trivial or low-value tests merely to increase it.
4. If `cargo llvm-cov` is unavailable, report the missing tool and give the installation command (`cargo install cargo-llvm-cov`); do not install tooling or change project dependencies without the user's approval.

Report tests added or updated, harness/mock design decisions, commands run and outcomes, the location of the LCOV report when generated, and any remaining meaningful coverage gaps.