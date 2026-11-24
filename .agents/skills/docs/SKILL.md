---
name: docs
description: Review and update documentation for changed Rust CLI behavior, configuration, public APIs, and releases while keeping examples accurate and testable. Use only when explicitly invoked as /docs.
disable-model-invocation: true
---

# Documentation Workflow

Inspect the requested change and its user-facing surfaces before editing documentation. Update only documentation that the behavior change makes inaccurate, incomplete, or newly necessary.

Check as applicable:

- `README.md`: installation, quick start, usage, configuration, and examples.
- CLI help and command examples: flags, arguments, defaults, exit behavior, and errors.
- Rust public APIs: rustdoc for externally relevant items, constraints, errors, and examples.
- `Changelog.md`: user-visible fixes, features, breaking changes, migration notes, and acknowledgements according to its existing format.

Derive commands and examples from real behavior; do not invent flags, output, paths, or configuration. Prefer concise task-oriented prose. When feasible, execute documented commands or a focused test to validate examples.

Do not alter version numbers, create releases, tags, or commits unless explicitly requested. Report documentation updated, validation performed, and remaining documentation gaps.