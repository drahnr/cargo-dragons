use anyhow::Context;
use cargo::{
    GlobalContext,
    core::{SourceId, Workspace, compiler::CompileMode, package::Package, resolver::CliFeatures},
    util::{auth, context::homedir},
};
use cargo_credential::{Operation, Secret};
use cargo_util_terminal::{Shell, Verbosity};
use regex::Regex;
use semver::Version;
use std::{collections::HashSet, fs, path::PathBuf, str::FromStr};
use toml_edit::Value;

use crate::{
    commands::{self, IndependenceCtx},
    util::{
        available_package_names, handle_empty_package_is_failures_with_available,
        make_pkg_predicate, members_deep,
    },
};

fn parse_regex(src: &str) -> Result<Regex, anyhow::Error> {
    Regex::new(src).context("Parsing Regex failed")
}

fn parse_compile_mode_str(src: &str) -> anyhow::Result<CompileMode> {
    Ok(match src {
        "build" => CompileMode::Build,
        "test" => CompileMode::Test,
        "check" => CompileMode::Check { test: false },
        _ => anyhow::bail!(
            "Only `build`, `test`, `check` are known compilation modes, provided is unknown: {}",
            src
        ),
    })
}

fn package_list_with_versions(packages: &[Package]) -> String {
    Vec::from_iter(
        packages
            .iter()
            .map(|p| format!("{} ({})", p.name(), p.version())),
    )
    .join(", ")
}

fn print_available_packages(gctx: &GlobalContext, ws: &Workspace<'_>) {
    for name in available_package_names(gctx, ws) {
        println!("{name}");
    }
}

fn version_command_pkg_opts(cmd: &VersionCommand) -> Option<&PackageSelectOptions> {
    Some(match cmd {
        VersionCommand::Release { pkg_opts, .. }
        | VersionCommand::BumpBreaking { pkg_opts, .. }
        | VersionCommand::BumpToDev { pkg_opts, .. }
        | VersionCommand::BumpPre { pkg_opts, .. }
        | VersionCommand::BumpPatch { pkg_opts, .. }
        | VersionCommand::BumpMinor { pkg_opts, .. }
        | VersionCommand::BumpMajor { pkg_opts, .. }
        | VersionCommand::Set { pkg_opts, .. }
        | VersionCommand::SetPre { pkg_opts, .. }
        | VersionCommand::SetBuild { pkg_opts, .. } => pkg_opts,
    })
}

fn command_pkg_opts(cmd: &Command) -> Option<&PackageSelectOptions> {
    match cmd {
        Command::Set { pkg_opts, .. }
        | Command::AddOwner { pkg_opts, .. }
        | Command::CleanDeps { pkg_opts, .. }
        | Command::DeDevDeps { pkg_opts }
        | Command::ToRelease { pkg_opts, .. }
        | Command::Check { pkg_opts, .. }
        | Command::Unleash { pkg_opts, .. }
        | Command::UnifyDeps { pkg_opts }
        | Command::IndependenceCheck { pkg_opts, .. } => Some(pkg_opts),
        #[cfg(feature = "gen-readme")]
        Command::GenReadme { pkg_opts, .. } => Some(pkg_opts),
        Command::Version { cmd } => version_command_pkg_opts(cmd),
        Command::Completions { .. } | Command::Rename { .. } => None,
    }
}

fn report_already_published_versions(
    gctx: &GlobalContext,
    packages: &[Package],
) -> anyhow::Result<()> {
    if !packages.is_empty() {
        let verb = if packages.len() == 1 { "is" } else { "are" };
        gctx.shell().status(
            "Already published",
            format!(
                "{} {verb} already present at the same version on the registry",
                package_list_with_versions(packages)
            ),
        )?;
    }
    Ok(())
}

fn handle_empty_release_plan(
    gctx: &GlobalContext,
    packages: &[Package],
    already_published: &[Package],
    empty_package_is_failure: bool,
    available_packages: Option<&[String]>,
) -> anyhow::Result<bool> {
    report_already_published_versions(gctx, already_published)?;

    if packages.is_empty() && !already_published.is_empty() {
        if empty_package_is_failure {
            anyhow::bail!(
                "All selected packages are already present on the registry at the same version. Exiting"
            );
        }

        println!(
            "All selected packages are already present on the registry at the same version. All good. Exiting."
        );
        return Ok(true);
    }

    handle_empty_package_is_failures_with_available(
        packages,
        empty_package_is_failure,
        available_packages,
    )
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerateReadmeMode {
    // Generate Readme only if it is missing.
    IfMissing,
    // Generate Readme & append to existing file.
    Append,
    // Generate Readme & overwrite/replace the existing file.
    Replace,
}

#[derive(clap::Parser, Debug)]
pub struct PackageSelectOptions {
    /// Only use the specific set of packages.
    ///
    /// Accepts repeated flags (`-p a -p b`) and comma-separated values (`-p a,b`). This is
    /// mutually exclusive with skip and ignore-version-pre.
    #[clap(short, long, value_parser = parse_regex, value_delimiter = ',')]
    pub packages: Vec<Regex>,

    /// Skip the package names matching ...
    ///
    /// Provide one or many regular expression that, if the package name matches, means we skip
    /// that package. Mutually exclusive with `--packages`
    #[clap(short, long, value_parser = parse_regex)]
    pub skip: Vec<Regex>,

    /// Ignore version pre-releases
    ///
    /// Skip if the SemVer pre-release field is any of the listed. Mutually exclusive with
    /// `--packages`
    #[clap(short, long)]
    pub ignore_pre_version: Vec<String>,

    /// Ignore whether `publish` is set.
    ///
    /// If nothing else is specified, `publish = true` is assumed for every package. If publish
    /// is set to false or any registry, it is ignored by default. If you want to include it
    /// regardless, set this flag.
    #[clap(long)]
    pub ignore_publish: bool,

    /// Automatically detect the packages, which changed compared to the given git commit.
    ///
    /// Compares the current git `head` to the reference given, identifies which files changed
    /// and attempts to identify the packages and its dependents through that mechanism. You
    /// can use any `tag`, `branch` or `commit`, but you must be sure it is available
    /// (and up to date) locally.
    #[clap(short = 'c', long = "changed-since")]
    pub changed_since: Option<String>,

    /// Even if not selected by default, also include depedencies with a pre (cascading)
    #[clap(long)]
    pub include_pre_deps: bool,

    /// List available packages and exit.
    #[clap(long)]
    pub list_packages: bool,
}

#[derive(clap::Parser, Debug)]
pub struct ReleasePlanOptions {
    /// Consider no package matching the criteria an error
    #[arg(long)]
    empty_package_is_failure: bool,

    /// Write a graphviz dot file to the given destination
    #[arg(long = "dot-graph")]
    dot_graph: Option<PathBuf>,
}

#[derive(clap::Parser, Debug)]
pub struct VerificationOptions {
    /// Actually build the package during verification.
    ///
    /// By default, this only runs `cargo check` against the package build. Set this flag to have it
    /// run an actual `build` instead.
    #[arg(long)]
    build: bool,

    /// Generate & verify whether the Readme file has changed.
    ///
    /// When enabled, this will generate a Readme file from the crate's doc comments (using
    /// cargo-readme), and check whether the existing Readme (if any) matches.
    #[arg(long)]
    check_readme: bool,
}

#[derive(clap::Subcommand, Debug)]
pub enum VersionCommand {
    /// Pick pre-releases and put them to release mode.
    Release {
        #[command(flatten)]
        pkg_opts: PackageSelectOptions,
        /// Force an update of dependencies
        ///
        /// Hard set to the new version, do not check whether the given one still matches
        #[arg(long)]
        force_update: bool,
    },
    /// Smart bumping of crates for the next breaking release, bumps minor for 0.x and major for
    /// major > 1
    BumpBreaking {
        #[command(flatten)]
        pkg_opts: PackageSelectOptions,
        /// Force an update of dependencies
        ///
        /// Hard set to the new version, do not check whether the given one still matches
        #[arg(long)]
        force_update: bool,
    },
    /// Smart bumping of crates for the next breaking release and add a `-dev`-pre-release-tag
    BumpToDev {
        #[command(flatten)]
        pkg_opts: PackageSelectOptions,
        /// Force an update of dependencies
        ///
        /// Hard set to the new version, do not check whether the given one still matches
        #[arg(long)]
        force_update: bool,
        /// Use this identifier instead of `dev`  for the pre-release
        #[arg(long)]
        pre_tag: Option<String>,
    },
    /// Increase the pre-release suffix, keep prefix, set to `.1` if no suffix is present
    BumpPre {
        #[command(flatten)]
        pkg_opts: PackageSelectOptions,
        /// Force an update of dependencies
        ///
        /// Hard set to the new version, do not check whether the given one still matches
        #[arg(long)]
        force_update: bool,
    },
    /// Increase the patch version, unset prerelease
    BumpPatch {
        #[command(flatten)]
        pkg_opts: PackageSelectOptions,
        /// Force an update of dependencies
        ///
        /// Hard set to the new version, do not check whether the given one still matches
        #[arg(long)]
        force_update: bool,
    },
    /// Increase the minor version, unset prerelease and patch
    BumpMinor {
        #[command(flatten)]
        pkg_opts: PackageSelectOptions,
        /// Force an update of dependencies
        ///
        /// Hard set to the new version, do not check whether the given one still matches
        #[arg(long)]
        force_update: bool,
    },
    /// Increase the major version, unset prerelease, minor and patch
    BumpMajor {
        #[command(flatten)]
        pkg_opts: PackageSelectOptions,
        /// Force an update of dependencies
        ///
        /// Hard set to the new version, do not check whether the given one still matches
        #[arg(long)]
        force_update: bool,
    },
    /// Hard set version to given string
    Set {
        #[command(flatten)]
        pkg_opts: PackageSelectOptions,
        /// Set to a specific Version
        version: Version,
        /// Force an update of dependencies
        ///
        /// Hard set to the new version, do not check whether the given one still matches
        #[arg(long)]
        force_update: bool,
    },
    /// Set the pre-release to string
    SetPre {
        #[command(flatten)]
        pkg_opts: PackageSelectOptions,
        /// The string to set the pre-release to
        #[arg()]
        pre: String,
        /// Force an update of dependencies
        ///
        /// Hard set to the new version, do not check whether the given one still matches
        #[arg(long)]
        force_update: bool,
    },
    /// Set the metadata to string
    SetBuild {
        #[command(flatten)]
        pkg_opts: PackageSelectOptions,
        /// The specific metadata to set to
        #[arg()]
        meta: String,
        /// Force an update of dependencies
        ///
        /// Hard set to the new version, do not check whether the given one still matches
        #[arg(long)]
        force_update: bool,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum Command {
    /// Generate the clap completions
    Completions {
        #[arg(short, long, default_value = "zsh")]
        shell: clap_complete::Shell,
    },
    /// Set a field in all manifests
    ///
    /// Go through all matching crates and set the field name to value.
    /// Add the field if it doesn't exists yet.
    Set {
        #[command(flatten)]
        pkg_opts: PackageSelectOptions,
        /// The root key table to look the key up in
        #[arg(short, long, default_value = "package")]
        root_key: String,
        /// Name of the field
        name: String,
        /// Value to set it, too
        value: String,
    },
    /// Rename a package
    ///
    /// Update the internally used references to the package by adding an `package = ` entry
    /// to the dependencies.
    Rename {
        /// Name of the field
        old_name: String,
        /// Value to set it, too
        new_name: String,
    },
    /// Messing with versioning
    ///
    /// Change versions as requested, then update all package's dependencies
    /// to ensure they are still matching
    Version {
        #[command(subcommand)]
        cmd: VersionCommand,
    },
    /// Add owners for a lot of crates
    AddOwner {
        #[command(flatten)]
        pkg_opts: PackageSelectOptions,
        /// Owner to add to the packages
        owner: String,
        /// the crates.io token to use for API access
        ///
        /// If neither this nor Cargo's native token environment variable is set,
        /// this falls back to the default value provided in the user directory.
        #[arg(long, env = "CARGO_REGISTRY_TOKEN", hide_env_values = true)]
        token: Option<String>,
    },
    /// Check the package(s) for unused dependencies
    CleanDeps {
        #[command(flatten)]
        pkg_opts: PackageSelectOptions,
        /// Do only check if you'd clean up.
        ///
        /// Abort if you found unused dependencies. `--check` is kept as a backwards-compatible
        /// alias for `--check-only`.
        #[arg(long = "check-only", alias = "check")]
        check_only: bool,
    },
    /// Deprecated: deactivate `[dev-dependencies]` in matching package manifests.
    ///
    /// This mutates manifests in-place and is kept only for compatibility. Prefer `check` or
    /// `unleash`, which perform release verification without relying on this standalone step.
    #[command(name = "de-dev-deps")]
    DeDevDeps {
        #[command(flatten)]
        pkg_opts: PackageSelectOptions,
    },
    /// Calculate the packages and the order in which to release
    ///
    /// Go through the members of the workspace and calculate the dependency tree. Halt early
    /// if any circles are found
    ToRelease {
        #[command(flatten)]
        pkg_opts: PackageSelectOptions,
        #[command(flatten)]
        release_opts: ReleasePlanOptions,
    },
    /// Check whether crates can be packaged
    ///
    /// Package the selected packages, then check the packages can be build with
    /// the packages as dependencies as to be released.
    Check {
        #[command(flatten)]
        pkg_opts: PackageSelectOptions,
        #[command(flatten)]
        verify_opts: VerificationOptions,
        #[command(flatten)]
        release_opts: ReleasePlanOptions,
    },
    /// Generate Readme files
    ///
    /// Generate Readme files for the selected packges, based
    /// on the crates' doc comments.
    #[cfg(feature = "gen-readme")]
    GenReadme {
        #[command(flatten)]
        pkg_opts: PackageSelectOptions,
        /// Generate readme file for package.
        ///
        /// Depending on the chosen option, this will generate a Readme
        /// file from the crate's doc comments (using cargo-readme).
        #[arg(long)]
        readme_mode: GenerateReadmeMode,
        /// Consider no package matching the criteria an error
        #[arg(long)]
        empty_package_is_failure: bool,
    },
    /// Unleash 'em dragons
    ///
    /// Package all selected crates, check them and attempt to publish them.
    Unleash {
        #[command(flatten)]
        pkg_opts: PackageSelectOptions,
        #[command(flatten)]
        verify_opts: VerificationOptions,
        #[command(flatten)]
        release_opts: ReleasePlanOptions,
        /// dry run
        #[arg(long)]
        dry_run: bool,
        /// Skip package verification before release.
        ///
        /// By default, verification runs even with `--dry-run`. `--no-check` is kept as a
        /// backwards-compatible alias for `--skip-verify`.
        #[arg(long = "skip-verify", alias = "no-check")]
        no_check: bool,
        /// Do not publish explicitly selected packages themselves; publish only selected dependencies.
        ///
        /// By default, packages selected with `-p/--packages` are published too.
        #[arg(long)]
        without_self: bool,
        /// Ensure we have the owner set as well
        #[arg(long = "owner")]
        add_owner: Option<String>,
        /// the crates.io token to use for uploading
        ///
        /// If neither this nor Cargo's native token environment variable is set,
        /// this falls back to the default value provided in the user directory.
        #[arg(long, env = "CARGO_REGISTRY_TOKEN", hide_env_values = true)]
        token: Option<String>,
    },
    /// Unify all dependencies to those used in the workspace
    /// and suggest additional ones.
    UnifyDeps {
        #[command(flatten)]
        pkg_opts: PackageSelectOptions,
    },
    /// Check whether packages can be build independently
    ///
    /// Ensure all packages can be build not only as part of the workspace
    /// with workspace joint dependency and feature resolution, but also with per package
    /// compilation
    IndependenceCheck {
        /// Specifiy one of the three check modes:
        ///
        /// "test" - Building the tests for the symbols.
        /// "build" - Building a target with rustc (lib or bin).
        /// "check" - Building a target with rustc to emit rmeta metadata only.
        #[arg(long, default_value="test", value_parser = parse_compile_mode_str)]
        mode: Vec<CompileMode>,

        /// Define the context in which check should be executed:
        ///
        /// "ephemeral" - which would use a temporary package as a context.
        ///
        /// "inplace" - which will perform the necessary compilations in the package directory.
        #[arg(long="ctx", default_value_t = IndependenceCtx::default(), value_parser = IndependenceCtx::from_str)]
        context: IndependenceCtx,

        #[command(flatten)]
        pkg_opts: PackageSelectOptions,

        /// Do not attempt to compile all packages, but fail at the first one that doesn't pass the
        /// test.
        #[arg(long)]
        failfast: bool,
    },
}

#[derive(Debug, clap::Parser)]
#[command(version, about = "Release the crates of this massiv monorepo")]
pub struct Args {
    /// The path to workspace manifest
    ///
    /// Can either be the folder if the file is named `Cargo.toml` or the path
    /// to the specific `.toml`-manifest to load as the cargo workspace.
    #[arg(short, long, value_parser=PathBuf::from_str, default_value = ".", value_hint = clap::ValueHint::AnyPath)]
    #[clap(short, long, global(true))]
    pub manifest_path: PathBuf,

    // TODO consider using these  instead of custom parsin
    // #[command(flatten)]
    // manifest: clap_cargo::Manifest,
    // #[command(flatten)]
    // workspace: clap_cargo::Workspace,
    // #[command(flatten)]
    // features: clap_cargo::Features,
    #[command(flatten)]
    pub verbosity: clap_verbosity_flag::Verbosity<clap_verbosity_flag::InfoLevel>,

    #[command(subcommand)]
    pub cmd: Command,
}

fn verify_readme_feature() -> anyhow::Result<()> {
    if cfg!(feature = "gen-readme") {
        Ok(())
    } else {
        anyhow::bail!(
            "Readme related functionalities not available. Please re-install with gen-readme feature."
        )
    }
}

fn ensure_crates_io_login(
    gctx: &GlobalContext,
    token: Option<&Secret<String>>,
) -> Result<(), anyhow::Error> {
    let source_id = SourceId::crates_io(gctx)?;
    if let Some(token) = token {
        // Seeds Cargo's in-memory credential cache for this process; this does not
        // persist or overwrite the user's credentials.toml.
        auth::cache_token_from_commandline(gctx, &source_id, Secret::as_deref(token));
    }

    auth::auth_token(gctx, &source_id, None, Operation::Read, Vec::new(), false)
        .map(|_| ())
        .with_context(|| {
            "Not logged in to crates.io. Run `cargo login` or provide a token with \
            `--token`/`CARGO_REGISTRY_TOKEN` before running `cargo dragons unleash` \
            (including `--dry-run`)."
        })
}

//TODO: Refactor this implementation to be a bit more readable.
pub fn run(args: Args) -> Result<(), anyhow::Error> {
    pretty_env_logger::init();

    let cwd = args.manifest_path.parent().unwrap().to_path_buf();
    let cargo_home = homedir(&cwd).context(
        "Cargo couldn't find your home directory. This probably means that $HOME was not set.",
    )?;
    let gctx = GlobalContext::new(Shell::new(), cwd, cargo_home);
    gctx.values()?;
    gctx.load_credentials()?;

    gctx.shell().set_verbosity(
        match args.verbosity.log_level().unwrap_or(log::Level::Error) {
            log::Level::Trace | log::Level::Debug => Verbosity::Verbose,
            log::Level::Info => Verbosity::Normal,
            log::Level::Warn => Verbosity::Normal,
            log::Level::Error => Verbosity::Quiet,
        },
    );

    let root_manifest = {
        let mut path = args.manifest_path.clone();
        if path.is_dir() {
            path = path.join("Cargo.toml")
        }
        fs::canonicalize(path)?
    };

    let mut ws = Workspace::new(&root_manifest, &gctx).context("Reading workspace failed")?;

    if command_pkg_opts(&args.cmd).is_some_and(|pkg_opts| pkg_opts.list_packages) {
        print_available_packages(&gctx, &ws);
        return Ok(());
    }

    //TODO: Seperate matching from Command implementations to make this a more readable codebase
    match args.cmd {
        Command::Completions { shell } => {
            let sink = &mut std::io::stdout();
            let mut app = <Args as clap::CommandFactory>::command();
            let app = &mut app;
            clap_complete::generate(shell, app, app.get_name().to_string(), sink);
            Ok(())
        }
        Command::CleanDeps {
            pkg_opts,
            check_only,
        } => {
            let predicate = make_pkg_predicate(&gctx, &ws, pkg_opts)?;
            commands::clean_up_unused_dependencies(&gctx, &ws, predicate, check_only)
        }
        Command::DeDevDeps { pkg_opts } => {
            gctx.shell().warn(
                "`de-dev-deps` is deprecated; prefer `check` or `unleash` for release verification.",
            )?;
            let predicate = make_pkg_predicate(&gctx, &ws, pkg_opts)?;
            commands::deactivate_dev_dependencies(ws.members().filter(|p| predicate(p)))
        }
        Command::AddOwner {
            owner,
            token,
            pkg_opts,
        } => {
            let token = token.map(Secret::from);
            let predicate = make_pkg_predicate(&gctx, &ws, pkg_opts)?;

            for pkg in ws.members().filter(|p| predicate(p)) {
                commands::add_owner(&gctx, pkg, owner.clone(), token.clone())?;
            }
            Ok(())
        }
        Command::Set {
            root_key,
            name,
            value,
            pkg_opts,
        } => {
            if name == "name" {
                anyhow::bail!("To change the name please use the rename command!");
            }
            let predicate = make_pkg_predicate(&gctx, &ws, pkg_opts)?;
            let type_value =
                if let Ok(v) = bool::from_str(&value).map_err(|_| i64::from_str(&value)) {
                    Value::from(v)
                } else {
                    Value::from(value)
                };

            commands::set_field(
                ws.members().filter(|p| {
                    predicate(p) && gctx.shell().status("Setting on", p.name()).is_ok()
                }),
                root_key,
                name,
                type_value,
            )
        }
        Command::UnifyDeps { pkg_opts } => {
            let predicate = make_pkg_predicate(&gctx, &ws, pkg_opts)?;
            commands::unify_dependencies(&gctx, &mut ws, predicate)?;
            Ok(())
        }
        Command::Rename { old_name, new_name } => {
            let predicate = |p: &Package| p.name().to_string().trim() == old_name;
            let renamer = |_p: &Package| Some(new_name.clone());

            commands::rename(&gctx, &ws, predicate, renamer)
        }
        Command::Version { cmd } => {
            commands::adjust_version(&gctx, &ws, cmd)?;
            Ok(())
        }
        Command::ToRelease {
            pkg_opts,
            release_opts,
        } => {
            let available_packages = pkg_opts
                .packages
                .is_empty()
                .then(|| available_package_names(&gctx, &ws));
            let predicate = make_pkg_predicate(&gctx, &ws, pkg_opts)?;

            let plan = commands::release_plan(&gctx, &ws, predicate, release_opts.dot_graph)?;
            if handle_empty_release_plan(
                &gctx,
                &plan.to_release,
                &plan.already_published,
                release_opts.empty_package_is_failure,
                available_packages.as_deref(),
            )? {
                return Ok(());
            }

            println!("{:}", package_list_with_versions(&plan.to_release));

            Ok(())
        }
        Command::Check {
            pkg_opts,
            verify_opts,
            release_opts,
        } => {
            if verify_opts.check_readme {
                verify_readme_feature()?;
            }

            let available_packages = pkg_opts
                .packages
                .is_empty()
                .then(|| available_package_names(&gctx, &ws));
            let predicate = make_pkg_predicate(&gctx, &ws, pkg_opts)?;

            let plan = commands::release_plan(&gctx, &ws, predicate, release_opts.dot_graph)?;
            if handle_empty_release_plan(
                &gctx,
                &plan.to_release,
                &plan.already_published,
                release_opts.empty_package_is_failure,
                available_packages.as_deref(),
            )? {
                return Ok(());
            }

            commands::check_packages(
                &gctx,
                &plan.to_release,
                &ws,
                verify_opts.build,
                verify_opts.check_readme,
            )
        }
        #[cfg(feature = "gen-readme")]
        Command::GenReadme {
            pkg_opts,
            readme_mode,
            empty_package_is_failure,
        } => {
            let available_packages = pkg_opts
                .packages
                .is_empty()
                .then(|| available_package_names(&gctx, &ws));
            let predicate = make_pkg_predicate(&gctx, &ws, pkg_opts)?;

            let plan = commands::release_plan(&gctx, &ws, predicate, None)?;
            if handle_empty_release_plan(
                &gctx,
                &plan.to_release,
                &plan.already_published,
                empty_package_is_failure,
                available_packages.as_deref(),
            )? {
                return Ok(());
            }

            commands::gen_all_readme(&gctx, plan.to_release, &ws, readme_mode)
        }

        Command::Unleash {
            dry_run,
            no_check,
            token,
            add_owner,
            verify_opts,
            release_opts,
            pkg_opts,
            without_self,
        } => {
            gctx.shell().status("Checking", "crates.io login")?;
            let token = token.map(Secret::from);
            ensure_crates_io_login(&gctx, token.as_ref())?;

            let explicitly_selected = (!pkg_opts.packages.is_empty())
                .then(|| {
                    HashSet::from_iter(members_deep(&gctx, &ws).into_iter().filter_map(|package| {
                        pkg_opts
                            .packages
                            .iter()
                            .any(|selector| selector.is_match(&package.name()))
                            .then(|| package.name().as_str().to_owned())
                    }))
                })
                .unwrap_or_default();
            let available_packages = pkg_opts
                .packages
                .is_empty()
                .then(|| available_package_names(&gctx, &ws));
            let predicate = make_pkg_predicate(&gctx, &ws, pkg_opts)?;

            let plan = commands::release_plan(&gctx, &ws, predicate, release_opts.dot_graph)?;
            if handle_empty_release_plan(
                &gctx,
                &plan.to_release,
                &plan.already_published,
                release_opts.empty_package_is_failure,
                available_packages.as_deref(),
            )? {
                return Ok(());
            }

            let mut packages = plan.to_release;
            if !explicitly_selected.is_empty() {
                let to_release = HashSet::<String>::from_iter(
                    packages
                        .iter()
                        .map(|package| package.name().as_str().to_owned()),
                );

                if without_self {
                    packages
                        .retain(|package| !explicitly_selected.contains(package.name().as_str()));
                } else if explicitly_selected.is_disjoint(&to_release) && !packages.is_empty() {
                    let mut selected = explicitly_selected.iter().cloned().collect::<Vec<_>>();
                    selected.sort();
                    anyhow::bail!(
                        "Refusing to unleash only dependencies. \
                        None of the explicitly selected package(s) are in the release set: {}.\
                        Bump their version if they should be published, or run again with `--without-self`\
                        if you only intended to publish dependencies.",
                        selected.join(", ")
                    );
                }
            }

            if handle_empty_package_is_failures_with_available(
                &packages,
                release_opts.empty_package_is_failure,
                available_packages.as_deref(),
            )? {
                return Ok(());
            }

            if !no_check {
                if verify_opts.check_readme {
                    verify_readme_feature()?;
                }

                commands::check_packages(
                    &gctx,
                    &packages,
                    &ws,
                    verify_opts.build,
                    verify_opts.check_readme,
                )?;
            }

            gctx.shell()
                .status("Releasing", package_list_with_versions(&packages))?;

            commands::release(&gctx, packages, ws, dry_run, token, add_owner)
        }
        Command::IndependenceCheck {
            mode: modes,
            context,
            pkg_opts,
            failfast,
        } => {
            let predicate = make_pkg_predicate(&gctx, &ws, pkg_opts)?;

            let packages = Vec::<Package>::from_iter(
                members_deep(&gctx, &ws)
                    .iter()
                    .filter(|p| predicate(p))
                    .cloned(),
            );
            let opts = cargo::ops::PackageOpts {
                gctx: &gctx,
                verify: false,
                check_metadata: false,
                list: false,
                fmt: cargo::ops::PackageMessageFormat::Human,
                allow_dirty: true,
                include_lockfile: true,
                jobs: None,
                to_package: cargo::ops::Packages::Default,
                targets: Default::default(),
                cli_features: CliFeatures {
                    features: Default::default(),
                    all_features: false,
                    uses_default_features: true,
                },
                keep_going: !failfast,
                reg_or_index: None,
                dry_run: false,
            };

            commands::independence_check(&gctx, packages, &opts, ws, modes, context)
        }
    }
}
