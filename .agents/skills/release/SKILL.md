---
name: release
description: Prepare and review a safe cargo-dragons release - versioning, changelog, validation, and release artifacts, without publishing or mutating git unless explicitly authorized. Use only when explicitly invoked as /release.
disable-model-invocation: true
---

# Release Preparation

Treat release operations as high impact. Inspect the current version, git status, changelog, repository conventions, and requested release scope before changing anything.

Prepare a release checklist covering:

1. Intended version and semantic-versioning impact.
2. User-visible release notes in `Changelog.md`, following the existing format.
3. Manifest metadata and any version references that require synchronization.
4. Validation: `cargo fmt --all -- --check`, `cargo test --workspace`, and `cargo clippy --workspace -- -D warnings`.
5. Package verification, when relevant: `cargo package --allow-dirty` only to verify packaging locally; inspect its outcome.

Do not run `cargo publish`, create a git commit, create/push a tag, edit release pages, or make network-affecting release actions unless the user explicitly authorizes the specific action. Before any authorized destructive or external action, restate the exact version, target, and command for confirmation.

Report the readiness checklist, modified files, validation results, and every action intentionally not performed.
