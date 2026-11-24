---
name: arch
description: Perform an architecture review of the Rust project or current change, with emphasis on boundaries, dependencies, and trait-based testability. Use only when explicitly invoked as /arch.
disable-model-invocation: true
---

# Architecture Review

Assess the requested area or current change at the architectural level before recommending changes. Inspect relevant modules, public APIs, dependency directions, and tests.

Evaluate:

- Separation of domain logic, infrastructure, orchestration, and I/O.
- Dependency direction, coupling, ownership of responsibilities, and API boundaries.
- Error handling and state-management boundaries.
- Testability: production and test/mock variations must use focused traits with concrete implementations, not enums that select or wrap those alternatives.
- Whether abstractions are justified and appropriately small; avoid speculative generalization.

Present findings in priority order. For each actionable recommendation, explain the problem, impact, and a pragmatic direction for change, referencing relevant files and symbols. Distinguish verified observations from inferences. Do not edit code unless the user explicitly requests implementation.