use anyhow::Context;
use cargo::{
	core::{package::Package, resolver::CliFeatures, Verbosity, Workspace},
	util::{command_prelude::CompileMode, config::Config as CargoConfig},
};
use cargo_credential::Secret;
use regex::Regex;
use semver::Version;
use std::{fs, path::PathBuf, str::FromStr};
use toml_edit::Value;

use crate::{
	commands::{self, IndependenceCtx},
	util::{handle_empty_package_is_failures, make_pkg_predicate, members_deep},
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
	/// Only use the specfic set of packages
	///
	/// Apply only to the packages named as defined. This is mutually exclusive with skip and
	/// ignore-version-pre.
	#[clap(short, long, value_parser = parse_regex)]
	pub packages: Vec<Regex>,

	/// Skip the package names matching ...
	///
	/// Provide one or many regular expression that, if the package name matches, means we skip
	/// that package. Mutually exclusive with `--package`
	#[clap(short, long, value_parser = parse_regex)]
	pub skip: Vec<Regex>,

	/// Ignore version pre-releases
	///
	/// Skip if the SemVer pre-release field is any of the listed. Mutually exclusive with
	/// `--package`
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
		/// If this is nor the environment variable are set, this falls
		/// back to the default value provided in the user directory
		#[arg(long, env = "CRATES_TOKEN", hide_env_values = true)]
		token: Option<String>,
	},
	/// Deactivate the `[dev-dependencies]`
	///
	/// Go through the workspace and remove the `[dev-dependencies]`-section from the package
	/// manifest for all packages matching.
	DeDevDeps {
		#[command(flatten)]
		pkg_opts: PackageSelectOptions,
	},
	/// Check the package(s) for unused dependencies
	CleanDeps {
		#[command(flatten)]
		pkg_opts: PackageSelectOptions,
		/// Do only check if you'd clean up.
		///
		/// Abort if you found unused dependencies
		#[arg(long = "check")]
		check_only: bool,
	},
	/// Calculate the packages and the order in which to release
	///
	/// Go through the members of the workspace and calculate the dependency tree. Halt early
	/// if any circles are found
	ToRelease {
		/// Do not disable dev-dependencies
		///
		/// By default we disable dev-dependencies before the run.
		#[arg(long = "include-dev-deps")]
		include_dev: bool,
		#[command(flatten)]
		pkg_opts: PackageSelectOptions,
		/// Consider no package matching the criteria an error
		#[arg(long)]
		empty_package_is_failure: bool,

		/// Write a graphviz dot of all crates to be release and their dependency relation
		/// to the given path.
		#[arg(long = "dot-graph")]
		dot_graph: Option<PathBuf>,
	},
	/// Check whether crates can be packaged
	///
	/// Package the selected packages, then check the packages can be build with
	/// the packages as dependencies as to be released.
	Check {
		/// Do not disable dev-dependencies
		///
		/// By default we disable dev-dependencies before the run.
		#[arg(long = "include-dev-deps")]
		include_dev: bool,
		#[command(flatten)]
		pkg_opts: PackageSelectOptions,
		/// Actually build the package
		///
		/// By default, this only runs `cargo check` against the package
		/// build. Set this flag to have it run an actual `build` instead.
		#[arg(long)]
		build: bool,
		/// Generate & verify whether the Readme file has changed.
		///
		/// When enabled, this will generate a Readme file from
		/// the crate's doc comments (using cargo-readme), and
		/// check whether the existing Readme (if any) matches.
		#[arg(long)]
		check_readme: bool,
		/// Consider no package matching the criteria an error
		#[arg(long)]
		empty_package_is_failure: bool,

		/// Write a graphviz dot file to the given destination
		#[arg(long = "dot-graph")]
		dot_graph: Option<PathBuf>,
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
		/// Do not disable dev-dependencies
		///
		/// By default we disable dev-dependencies before the run.
		#[arg(long = "include-dev-deps")]
		include_dev: bool,
		#[command(flatten)]
		pkg_opts: PackageSelectOptions,
		/// Actually build the package in check
		///
		/// By default, this only runs `cargo check` against the package
		/// build. Set this flag to have it run an actual `build` instead.
		#[arg(long)]
		build: bool,
		/// dry run
		#[arg(long)]
		dry_run: bool,
		/// dry run
		#[arg(long)]
		no_check: bool,
		/// Ensure we have the owner set as well
		#[arg(long = "owner")]
		add_owner: Option<String>,
		/// the crates.io token to use for uploading
		///
		/// If this is nor the environment variable are set, this falls
		/// back to the default value provided in the user directory
		#[arg(long, env = "CRATES_TOKEN", hide_env_values = true)]
		token: Option<String>,
		/// Generate & verify whether the Readme file has changed.
		///
		/// When enabled, this will generate a Readme file from
		/// the crate's doc comments (using cargo-readme), and
		/// check whether the existing Readme (if any) matches.
		#[arg(long)]
		check_readme: bool,
		/// Consider no package matching the criteria an error
		#[arg(long)]
		empty_package_is_failure: bool,

		/// Write a graphviz dot file to the given destination
		#[arg(long = "dot-graph")]
		dot_graph: Option<PathBuf>,
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
		/// "test" - Building the tests for the symbols, does not work after de-devdeps.
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
		anyhow::bail!("Readme related functionalities not available. Please re-install with gen-readme feature.")
	}
}

//TODO: Refactor this implementation to be a bit more readable.
pub fn run(args: Args) -> Result<(), anyhow::Error> {
	pretty_env_logger::init();

	let c = CargoConfig::default().expect("Couldn't create cargo config");
	c.values()?;
	c.load_credentials()?;

	let get_token = |t| -> Result<Option<Secret<String>>, anyhow::Error> {
		Ok(match t {
			None => c
				.get_string("registry.token")?
				.map(|token_json_val| Secret::from(token_json_val.val)),
			_ => t,
		})
	};

	c.shell()
		.set_verbosity(match args.verbosity.log_level().unwrap_or(log::Level::Error) {
			log::Level::Trace | log::Level::Debug => Verbosity::Verbose,
			log::Level::Info => Verbosity::Normal,
			log::Level::Warn => Verbosity::Normal,
			log::Level::Error => Verbosity::Quiet,
		});

	let root_manifest = {
		let mut path = args.manifest_path.clone();
		if path.is_dir() {
			path = path.join("Cargo.toml")
		}
		fs::canonicalize(path)?
	};

	let mut ws = Workspace::new(&root_manifest, &c).context("Reading workspace failed")?;

	let maybe_patch =
		|ws, shouldnt_patch, predicate: &dyn Fn(&Package) -> bool| -> anyhow::Result<Workspace> {
			if shouldnt_patch {
				return Ok(ws);
			}

			c.shell().status("Preparing", "Disabling Dev Dependencies")?;

			commands::deactivate_dev_dependencies(
				ws.members()
					.filter(|p| predicate(p) && c.shell().status("Patching", p.name()).is_ok()),
			)?;
			// assure to re-read the workspace, otherwise `fn to_release` will still find cycles
			// (rightfully so!)
			Workspace::new(&root_manifest, &c).context("Reading workspace failed")
		};
	//TODO: Seperate matching from Command implementations to make this a more readable codebase
	match args.cmd {
		Command::Completions { shell } => {
			let sink = &mut std::io::stdout();
			let mut app = <Args as clap::CommandFactory>::command();
			let app = &mut app;
			clap_complete::generate(shell, app, app.get_name().to_string(), sink);
			Ok(())
		},
		Command::CleanDeps { pkg_opts, check_only } => {
			let predicate = make_pkg_predicate(&ws, pkg_opts)?;
			commands::clean_up_unused_dependencies(&ws, predicate, check_only)
		},
		Command::AddOwner { owner, token, pkg_opts } => {
			let token = get_token(token.map(Secret::from))?;
			let predicate = make_pkg_predicate(&ws, pkg_opts)?;

			for pkg in ws.members().filter(|p| predicate(p)) {
				commands::add_owner(ws.config(), pkg, owner.clone(), token.clone())?;
			}
			Ok(())
		},
		Command::Set { root_key, name, value, pkg_opts } => {
			if name == "name" {
				anyhow::bail!("To change the name please use the rename command!");
			}
			let predicate = make_pkg_predicate(&ws, pkg_opts)?;
			let type_value =
				if let Ok(v) = bool::from_str(&value).map_err(|_| i64::from_str(&value)) {
					Value::from(v)
				} else {
					Value::from(value)
				};

			commands::set_field(
				ws.members()
					.filter(|p| predicate(p) && c.shell().status("Setting on", p.name()).is_ok()),
				root_key,
				name,
				type_value,
			)
		},
		Command::UnifyDeps { pkg_opts } => {
			let predicate = make_pkg_predicate(&ws, pkg_opts)?;
			commands::unify_dependencies(&mut ws, predicate)?;
			Ok(())
		},
		Command::Rename { old_name, new_name } => {
			let predicate = |p: &Package| p.name().to_string().trim() == old_name;
			let renamer = |_p: &Package| Some(new_name.clone());

			commands::rename(&ws, predicate, renamer)
		},
		Command::Version { cmd } => {
			commands::adjust_version(&ws, cmd)?;
			Ok(())
		},
		Command::DeDevDeps { pkg_opts } => {
			let predicate = make_pkg_predicate(&ws, pkg_opts)?;
			let _ = maybe_patch(ws, false, &predicate)?;
			Ok(())
		},
		Command::ToRelease { include_dev, pkg_opts, empty_package_is_failure, dot_graph } => {
			let predicate = make_pkg_predicate(&ws, pkg_opts)?;
			let ws = maybe_patch(ws, include_dev, &predicate)?;

			let packages = commands::packages_to_release(&ws, predicate, dot_graph)?;
			handle_empty_package_is_failures(&packages, empty_package_is_failure)?;

			println!(
				"{:}",
				Vec::from_iter(packages.iter().map(|p| format!("{} ({})", p.name(), p.version())))
					.join(", ")
			);

			Ok(())
		},
		Command::Check {
			include_dev,
			build,
			pkg_opts,
			check_readme,
			empty_package_is_failure,
			dot_graph,
		} => {
			if check_readme {
				verify_readme_feature()?;
			}

			let predicate = make_pkg_predicate(&ws, pkg_opts)?;
			let ws = maybe_patch(ws, include_dev, &predicate)?;

			let packages = commands::packages_to_release(&ws, predicate, dot_graph)?;
			handle_empty_package_is_failures(&packages, empty_package_is_failure)?;

			commands::check_packages(&packages, &ws, build, check_readme)
		},
		#[cfg(feature = "gen-readme")]
		Command::GenReadme { pkg_opts, readme_mode, empty_package_is_failure } => {
			let predicate = make_pkg_predicate(&ws, pkg_opts)?;
			let ws = maybe_patch(ws, false, &predicate)?;

			let packages = commands::packages_to_release(&ws, predicate, None)?;
			handle_empty_package_is_failures(&packages, empty_package_is_failure)?;

			commands::gen_all_readme(packages, &ws, readme_mode)
		},

		Command::Unleash {
			dry_run,
			no_check,
			token,
			include_dev,
			add_owner,
			build,
			pkg_opts,
			check_readme,
			empty_package_is_failure,
			dot_graph,
		} => {
			let predicate = make_pkg_predicate(&ws, pkg_opts)?;
			let ws = maybe_patch(ws, include_dev, &predicate)?;

			let packages = commands::packages_to_release(&ws, predicate, dot_graph)?;
			handle_empty_package_is_failures(&packages, empty_package_is_failure)?;

			if !no_check {
				if check_readme {
					verify_readme_feature()?;
				}

				commands::check_packages(&packages, &ws, build, check_readme)?;
			}

			ws.config().shell().status(
				"Releasing",
				Vec::from_iter(packages.iter().map(|p| format!("{} ({})", p.name(), p.version())))
					.join(", "),
			)?;

			let token = get_token(token.map(Secret::from))?;
			commands::release(packages, ws, dry_run, token, add_owner)
		},
		Command::IndependenceCheck { mode: modes, context, pkg_opts, failfast } => {
			let predicate = make_pkg_predicate(&ws, pkg_opts)?;

			let packages = Vec::<Package>::from_iter(
				members_deep(&ws).iter().filter(|p| predicate(p)).cloned(),
			);

			let opts = cargo::ops::PackageOpts {
				config: &c,
				verify: false,
				check_metadata: true,
				list: false,
				allow_dirty: true,
				jobs: None,
				to_package: cargo::ops::Packages::Default,
				targets: Default::default(),
				cli_features: CliFeatures {
					features: Default::default(),
					all_features: false,
					uses_default_features: true,
				},
				keep_going: !failfast,
			};

			commands::independence_check(packages, &opts, ws, modes, context)
		},
	}
}
