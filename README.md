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
            Ignore whether `publish` is set.

            If nothing else is specified, `publish = true` is assumed for every package. If publish is set to false or
            any registry, it is ignored by default. If you want to include it regardless, set this flag.
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
        --token <token>
            the crates.io token to use for uploading

            If this is nor the environment variable are set, this falls back to the default value provided in the user
            directory [env: CRATES_TOKEN]
```

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

## In the wild

_You are using the tooling and want to be mentioned here–[create an issue](https://github.com/gnunicorn/cargo-dragons/issues/new)_

- [Juice](https://github.com/spearow/juice)
- [fatality](https://github.com/drahnr/fatality)

## License & Credits

This Software is released under the [GNU General Public License (GPL) 3.0](https://www.gnu.org/licenses/gpl-3.0.en.html).

This, as any other software, is build on the shoulders of giants. In particular, this uses `cargo` internally and draws heavily on the knowledge established by [cargo publish-all](https://gitlab.com/torkleyy/cargo-publish-all) and [cargo hack](https://github.com/taiki-e/cargo-hack).
