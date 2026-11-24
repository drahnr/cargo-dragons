# cargo [dragons 🐉](https://github.com/drahnr/cargo-dragons)

`cargo` release automation tooling for _massiv mono-repo_.

## Changes

see [Changelog.md](./Changelog.md)

## Installation

Use `cargo install --path .` to install.

## Usage

Try and have it report what it would do on your mono repo with

```sh

cargo dragons unleash --dry-run
```

There are more options available on the CLI, just run with `--help`:

```sh
Release the crates of this massiv monorepo

USAGE:
    cargo-dragons [FLAGS] [OPTIONS] <SUBCOMMAND>

FLAGS:
    -h, --help
            Prints help information

    -V, --version
            Prints version information

    -v, --verbose
            Show verbose cargo output


OPTIONS:
    -l, --log <log>
            Specify the log levels [default: warn]

    -m, --manifest-path <manifest-path>
            The path to workspace manifest

            Can either be the folder if the file is named `Cargo.toml` or the path to the specific `.toml`-manifest to
            load as the cargo workspace. [default: ./]

SUBCOMMANDS:
    add-owner      Add owners for a lot of crates
    check          Check whether crates can be packaged
    clean-deps     Check the package(s) for unused dependencies
    de-dev-deps    Deactivate the `[dev-dependencies]`
    unleash        Unleash ’em dragons
    help           Prints this message or the help of the given subcommand(s)
    rename         Rename a package
    set            Set a field in all manifests
    to-release     Calculate the packages and the order in which to release
    version        Messing with versioning
```

### unleash

The main command is `cargo dragons unleash`, here is its help. All subcommands have extensive `--help` for you.

```sh
$ cargo-dragons unleash --help
Unleash ’em dragons

Package all selected crates, check them and attempt to publish them.

USAGE:
    cargo-dragons unleash [FLAGS] [OPTIONS]

FLAGS:
        --build
            Actually build the package in check

            By default, this only runs `cargo check` against the package build. Set this flag to have it run an actual
            `build` instead.
        --check-readme
            Generate & verify whether the Readme file has changed.

            When enabled, this will generate a Readme file from the crate’s doc comments (using cargo-readme), and check
            whether the existing Readme (if any) matches.
        --dry-run
            dry run

        --empty-package-is-failure
            Consider no package matching the criteria an error

    -h, --help
            Prints help information

        --ignore-publish
            Bypass the `package.publish` filter during package selection.

            Packages with a registry allowlist require this flag to be selected. Packages with publishing disabled
            (`publish = false` or `publish = []`) are never included in release plans, even with this flag.
        --include-dev-deps
            Do not disable dev-dependencies

            By default we disable dev-dependencies before the run.
        --include-pre-deps
            Even if not selected by default, also include depedencies with a pre (cascading)

        --no-check
            dry run

    -V, --version
            Prints version information


OPTIONS:
        --owner <add-owner>
            Ensure we have the owner set as well

    -c, --changed-since <changed-since>
            Automatically detect the packages, which changed compared to the given git commit.

            Compares the current git `head` to the reference given, identifies which files changed and attempts to
            identify the packages and its dependents through that mechanism. You can use any `tag`, `branch` or
            `commit`, but you must be sure it is available (and up to date) locally.
    -i, --ignore-pre-version <ignore-pre-version>...
            Ignore version pre-releases

            Skip if the SemVer pre-release field is any of the listed. Mutually exclusive with `--package`
    -p, --packages <packages>...
            Only use the specfic set of packages

            Apply only to the packages named as defined. This is mutually exclusive with skip and ignore-version-pre.
    -s, --skip <skip>...
            Skip the package names matching ...

            Provide one or many regular expression that, if the package name matches, means we skip that package.
            Mutually exclusive with `--package`
        --token <[REGISTRY:]TOKEN>
            Explicit token for a selected publication registry; repeat for multiple registries. Also accepts --token=...

            A bare token targets the sole selected registry, or crates-io if multiple registries are selected.
            Prefixes must name selected registries. Duplicate tokens for a registry and empty tokens are rejected.
            Each selected registry needs an explicit token or Cargo credentials, even during a dry run.
```

### First release of local dependencies

When releasing a package with unpublished path dependencies, use
`cargo dragons unleash -p PACKAGE --include-deps`. This adds publishable local
normal and build dependencies recursively, checks the packaged crates together,
and publishes their actual versions in dependency order. Exact versions already
on their publication registry are skipped. Dev-only dependencies are not added.
Use `to-release -p PACKAGE --include-deps` to preview the plan. A placeholder
prerelease does not satisfy a requirement for a stable version.

### Registry selection and authentication

The publication destination is resolved per package, consistently for login checks,
exact-version lookups, uploads, and `--owner` updates:

- Omitted `package.publish` or `publish = true`: crates.io.
- `publish = ["fff-rs"]`: that named registry. Use `--ignore-publish`
  to include packages with a registry allowlist in selection.
- `publish = false` or `publish = []`: never released.
- Multiple allowed registries: rejected as ambiguous; narrow the allowlist to one
  registry. There is currently no `--registry` override.

Alternate registries must be configured in Cargo, even when supplying explicit
CLI tokens. For example:

```toml
# .cargo/config.toml
[registries.fff-rs]
index = "sparse+https://registry.example.com/index/"
credential-provider = "cargo:token"
```

For Cargo-managed credentials, log in to the registry:

```sh
cargo login --registry fff-rs
cargo dragons unleash --ignore-publish --dry-run
```

Both `unleash` and `add-owner` accept repeated `--token [REGISTRY:]TOKEN`
arguments, including the `--token=...` form. Prefix examples are
`crates-io:$token` and `fff-rs:$other-token` (schematic token placeholders).
In real shell commands, use valid variable names and quote expansions. For a
selection targeting both crates.io and `fff-rs`:

```sh
cargo dragons unleash --ignore-publish \
    --token "crates-io:$token" --token="fff-rs:$other_token"
cargo dragons add-owner OWNER --ignore-publish \
    --token "crates-io:$token" --token="fff-rs:$other_token"
```

A registry prefix must name a selected publication registry; unknown prefixes and
configured-but-unused registry prefixes are rejected. A bare token remains
backwards compatible: it targets the sole selected publication registry when
there is only one, otherwise it targets `crates-io`, which must be selected.
Multiple tokens resolving to the same registry are rejected, including a bare
and a prefixed token for that registry. Empty tokens are rejected too.

Each selected registry must have either an explicit token or credentials available
through Cargo's credential provider, saved `cargo login`, or registry-specific
token environment variable. Cargo reads `CARGO_REGISTRY_TOKEN` for crates.io and
`CARGO_REGISTRIES_FFF_RS_TOKEN` for `fff-rs`. Explicit tokens can be combined with
Cargo-managed credentials for other selected registries. Both commands fail early
if any selected registry lacks credentials, including during `unleash --dry-run`.

Explicit tokens are process-local, are not persisted or written over saved
credentials, and are scoped to the matching registry's uploads and owner updates.
A crates.io token is not forwarded to an alternate registry. CLI tokens may be
visible in process listings; literal tokens may also remain in shell history.
Prefer Cargo credential providers or saved login credentials when that exposure
is a concern.

If crates.io source replacement is configured, an implicit crates.io destination
is rejected rather than silently publishing publicly; choose a single registry
explicitly in `package.publish`.

The registry regressions run with `cargo test --test registry`. Their harness
starts an ephemeral loopback sparse registry, uses an isolated `CARGO_HOME`, and
runs `cargo login` with dummy credentials. It verifies real uploads and owner
updates, exact-version detection, credential isolation, and disabled packages.
Crates.io access is redirected to the dummy server, so the tests never publish
outside the local harness or use your saved credentials.

## Common Usage Examples

**Release all crates** not having the `-dev`-pre version set

```sh
cargo-dragons unleash --ignore-pre-version dev
```

**Check if a PR can be released** (checking only changes in the PR compared to `main`)

```sh
cargo-dragons check --changed-since=main
```

**Release all crates** not having `test` in the name

```sh
cargo-dragons unleash --skip test
```

**Set the pre-version to `-dev`**

```sh
cargo-dragons version set-pre dev
```

**Bump the pre-version**, so for e.g. from `alpha.1` to `alpha.2` or `beta.3` to `beta.4`:

```sh
cargo-dragons version bump-pre
```

**Unify dependencies**, when having a larger workspace, sometimes multiple versions of dependencies sneak in, we want to unify.

```sh
cargo-dragons unify-deps
```

## Feature combinations in independence checks

`independence-check` checks defaults first, then every subset of the package's
feature names with automatic defaults disabled (`--no-default-features`). This
includes the empty set; explicitly selecting `default` enables its implied features.
Both in-place and ephemeral contexts use this policy; ordinary `check` retains defaults.
For `n` feature names, each mode runs `2^n + 1` checks. No combination limit is
currently enforced, so packages with many features can be expensive to check.

## In the wild

_You are using the tooling and want to be mentioned here–[create an issue](https://github.com/gnunicorn/cargo-dragons/issues/new)_

- [Juice](https://github.com/spearow/juice)
- [fatality](https://github.com/drahnr/fatality)

## License & Credits

This Software is released under the [GNU General Public License (GPL) 3.0](https://www.gnu.org/licenses/gpl-3.0.en.html).

This, as any other software, is build on the shoulders of giants. In particular, this uses `cargo` internally and draws heavily on the knowledge established by [cargo publish-all](https://gitlab.com/torkleyy/cargo-publish-all) and [cargo hack](https://github.com/taiki-e/cargo-hack).
