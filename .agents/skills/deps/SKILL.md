---
name: deps
description: Audit or update Rust dependencies safely, assessing compatibility and lockfile impact before validating workspace tests and clippy. Use only when explicitly invoked as /deps.
disable-model-invocation: true
---

# Dependency Maintenance

Inspect `Cargo.toml`, `Cargo.lock`, Rust toolchain constraints, and dependency usage before changing versions. Clarify the requested dependency, target version or goal, and compatibility constraints when they are not evident.

For an update or addition:

1. Prefer the smallest compatible change; preserve intentional exact pins and dependency groups.
2. Explain why the dependency is needed and avoid adding a dependency when existing code or dependencies suffice.
3. Update the manifest and lockfile through Cargo, not manual lockfile edits.
4. Inspect the manifest and lockfile diff for unexpected additions, removals, feature changes, or major-version upgrades.
5. Run focused tests, `cargo test --workspace` when practical, and `cargo clippy --workspace`.

For an audit, distinguish verified findings from unverified possibilities. Do not claim a dependency is vulnerable, compatible, maintained, or latest without evidence from the lockfile, package metadata, or an explicitly run audit tool.

Do not run broad upgrades, publish packages, change the Rust MSRV, or use networked tooling unless the user requested it or it is necessary for the requested update. Report dependency changes, compatibility considerations, lockfile impact, validation outcomes, and unresolved risks.