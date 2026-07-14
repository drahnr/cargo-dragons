#[cfg(feature = "gen-readme")]
use crate::commands::readme;

use crate::util::{DependencyAction, DependencyEntry, edit_each_dep};
use anyhow::Context;
use cargo::{
	GlobalContext,
	core::{
		Feature, SourceId, Workspace,
		compiler::{BuildConfig, CompileMode, UserIntent},
		package::Package,
		resolver::features::CliFeatures,
	},
	ops::{self, PackageOpts, package},
	sources::{PathSource, source::Source},
	util::{FileLock, OptVersionReq},
};
use flate2::read::GzDecoder;
use log::error;
use std::{
	collections::HashMap,
	fs::{read_to_string, write},
};
use tar::Archive;
use toml_edit::{DocumentMut, Item, Value};

fn user_intent(build_mode: CompileMode) -> anyhow::Result<UserIntent> {
	match build_mode {
		CompileMode::Build => Ok(UserIntent::Build),
		CompileMode::Test => Ok(UserIntent::Test),
		CompileMode::Check { test } => Ok(UserIntent::Check { test }),
		mode => anyhow::bail!("Unsupported compile mode: {mode:?}"),
	}
}

fn inject_replacement(
	pkg: &Package,
	replace: &HashMap<String, String>,
) -> Result<(), anyhow::Error> {
	let manifest = pkg.manifest_path();

	let document = read_to_string(manifest)?;
	let mut document = document.parse::<DocumentMut>()?;
	let root = document.as_table_mut();

	edit_each_dep(root, |name, _, entry, _| {
		if let Some(p) = replace.get(&name) {
			let path = Value::from(p.clone()).decorated(" ", " ");
			match entry {
				DependencyEntry::Inline(info) => {
					info.get_or_insert("path", path);
				},
				DependencyEntry::Table(info) => {
					info["path"] = Item::Value(path);
				},
			}
			DependencyAction::Mutated
		} else {
			DependencyAction::Untouched
		}
	});
	write(manifest, document.to_string().as_bytes()).context("Could not write local manifest")?;
	Ok(())
}

/// Checks the compilation of a single package for a given `build_mode`.
///
/// # Arguments:
/// `ws`: The global workspace of the package.
/// `package`: Information about the package to compile.
/// `opts`: Options for the compilation
/// `build_mode`: How to compile the package.
/// `features`: The crate features which to enable. Aka `cargo t --features "feat0 feat1"`
///
/// # Returns:
/// Ok, if the package has been successfully compiled.
/// Err otherwise.
pub(crate) fn run_check_inplace<'a>(
	gctx: &'a GlobalContext,
	ws: &Workspace<'a>,
	package: &Package,
	opts: &PackageOpts<'_>,
	build_mode: CompileMode,
	features: &[String],
) -> anyhow::Result<Workspace<'a>> {
	let workspace = Workspace::new(ws.root_manifest(), gctx)?;

	// Explicitly add the version to avoid conflicts, which would lead to an error.
	let explicit = format!("{}@{}", package.name(), package.version());

	ops::compile(
		&workspace,
		&ops::CompileOptions {
			build_config: BuildConfig::new(
				gctx,
				opts.jobs.clone(),
				false,
				&opts.targets,
				user_intent(build_mode)?,
			)?,
			spec: ops::Packages::Packages(vec![explicit]),
			cli_features: CliFeatures::from_command_line(features, false, true)?,
			filter: ops::CompileFilter::Default { required_features_filterable: true },
			target_rustdoc_args: None,
			target_rustc_args: None,
			rustdoc_document_private_items: false,
			honor_rust_version: false.into(),
			target_rustc_crate_types: None,
		},
	)?;

	Ok(workspace)
}

pub(crate) fn run_check_ephemeral<'a>(
	gctx: &'a GlobalContext,
	package: &Package,
	tar: &FileLock,
	opts: &PackageOpts<'_>,
	build_mode: CompileMode,
	replace: &HashMap<String, String>,
	features: &[String],
) -> anyhow::Result<Workspace<'a>> {
	let pkg = package;

	let f = GzDecoder::new(tar.file());
	let dst = tar.parent().join(format!("{}-{}", pkg.name(), pkg.version()));
	if dst.exists() {
		std::fs::remove_dir_all(&dst)?;
	}
	let mut archive = Archive::new(f);
	// We don't need to set the Modified Time, as it's not relevant to verification
	// and it errors on filesystems that don't support setting a modified timestamp
	archive.set_preserve_mtime(false);
	archive.unpack(dst.parent().expect("Expect Top level directory"))?;

	// Manufacture an ephemeral workspace to ensure that even if the top-level
	// package has a workspace we can still build our new crate.
	let (src, new_pkg) = {
		let id = SourceId::for_path(&dst)?;
		let mut src = PathSource::new(&dst, id, gctx);
		let new_pkg = src.root_package()?;

		// inject our local builds
		inject_replacement(&new_pkg, replace)?;

		// parse the manifest again
		let mut src = PathSource::new(&dst, id, gctx);
		let new_pkg = src.root_package()?;
		(src, new_pkg)
	};

	let pkg_fingerprint = src.fingerprint(&new_pkg)?;
	let ws = Workspace::ephemeral(new_pkg, gctx, None, true)?;

	let rustc_args =
		if pkg.manifest().unstable_features().require(Feature::public_dependency()).is_ok() {
			// FIXME: Turn this on at some point in the future
			//Some(vec!["-D exported_private_dependencies".to_string()])
			Some(Vec::new())
		} else {
			None
		};

	ops::compile(
		&ws,
		&ops::CompileOptions {
			build_config: BuildConfig::new(
				gctx,
				opts.jobs.clone(),
				false,
				&opts.targets,
				user_intent(build_mode)?,
			)?,
			spec: ops::Packages::Packages(vec![package.name().to_string()]),
			cli_features: CliFeatures::from_command_line(features, false, false)?,
			filter: ops::CompileFilter::Default { required_features_filterable: true },
			target_rustdoc_args: None,
			target_rustc_args: rustc_args,
			rustdoc_document_private_items: false,
			honor_rust_version: None,
			target_rustc_crate_types: None,
		},
	)?;

	// Check that `build.rs` didn't modify any files in the `src` directory.
	let ws_fingerprint = src.fingerprint(ws.current()?)?;
	if pkg_fingerprint != ws_fingerprint {
		anyhow::bail!(
			"Source directory was modified by build.rs during cargo publish. \
             Build scripts should not modify anything outside of OUT_DIR.\n\
             {path:?}\n\n\
             To proceed despite this, pass the `--no-verify` flag.",
			path = ws_fingerprint
		);
	}

	Ok(ws)
}

fn check_dependencies(package: &Package) -> Result<(), anyhow::Error> {
	let git_deps = Vec::from_iter(
		package
			.dependencies()
			.iter()
			.filter(|d| d.source_id().is_git() && d.version_req() == &OptVersionReq::Any)
			.map(|d| format!("{:}", d.package_name())),
	);
	if !git_deps.is_empty() {
		anyhow::bail!(
			"{}: has dependencies defined as git without a version: {:}",
			package.name(),
			git_deps.join(", ")
		)
	} else {
		Ok(())
	}
}

// ensure metadata is set
// https://doc.rust-lang.org/cargo/reference/publishing.html#before-publishing-a-new-crate
fn check_metadata(package: &Package) -> Result<(), anyhow::Error> {
	let metadata = package.manifest().metadata();
	let mut bad_fields = Vec::new();
	match metadata.description.as_deref() {
		Some("") => bad_fields.push("description is empty"),
		None => bad_fields.push("description is missing"),
		_ => {},
	}
	match metadata.repository.as_deref() {
		Some("") => bad_fields.push("repository is empty"),
		None => bad_fields.push("repository is missing"),
		_ => {},
	}
	match (metadata.license.as_ref(), metadata.license_file.as_ref()) {
		(Some(s), None) | (None, Some(s)) if !s.is_empty() => {},
		(Some(_), Some(_)) => bad_fields.push("You can't have license AND license_file"),
		_ => bad_fields.push("Neither license nor license_file is provided"),
	}
	if metadata.keywords.len() > 5 {
		bad_fields.push("crates.io only allows up to 5 keywords")
	}

	if bad_fields.is_empty() {
		Ok(())
	} else {
		anyhow::bail!("{}: Bad metadata: {}", package.name(), bad_fields.join("; "))
	}
}

#[cfg(feature = "gen-readme")]
fn check_readme<'a>(
	gctx: &GlobalContext,
	ws: &Workspace<'a>,
	pkg: &Package,
) -> Result<(), anyhow::Error> {
	let pkg_path = pkg.manifest_path().parent().expect("Folder exists");
	readme::check_pkg_readme(gctx, ws, pkg_path, pkg.manifest())
}

#[cfg(not(feature = "gen-readme"))]
fn check_readme(
	_gctx: &GlobalContext,
	_ws: &Workspace<'_>,
	_pkg: &Package,
) -> Result<(), anyhow::Error> {
	unreachable!()
}

pub fn check_packages(
	gctx: &GlobalContext,
	packages: &[Package],
	ws: &Workspace<'_>,
	build: bool,
	check_readme: bool,
) -> Result<(), anyhow::Error> {
	// FIXME: make build config configurable
	//        https://github.com/paritytech/cargo-unleash/issues/20
	let opts = PackageOpts {
		gctx,
		verify: false,
		check_metadata: true,
		list: false,
		fmt: ops::PackageMessageFormat::Human,
		allow_dirty: true,
		include_lockfile: true,
		jobs: None,
		to_package: ops::Packages::Default,
		targets: Default::default(),
		cli_features: CliFeatures {
			features: Default::default(),
			all_features: false,
			uses_default_features: true,
		},
		keep_going: false,
		reg_or_index: None,
		dry_run: true,
	};

	gctx.shell().status("Checking", "Metadata & Dependencies")?;

	let errors = packages.iter().fold(Vec::new(), |mut res, pkg| {
		if let Err(e) = check_metadata(pkg) {
			res.push(e);
		}
		if let Err(e) = check_dependencies(pkg) {
			res.push(e);
		}
		res
	});

	errors.iter().for_each(|s| error!("{:#?}", s));
	if !errors.is_empty() {
		anyhow::bail!("Soft checkes failed with {} errors (see above)", errors.len())
	}

	if check_readme {
		gctx.shell().status("Checking", "Readme files")?;
		let errors = packages.iter().fold(Vec::new(), |mut res, pkg| {
			if let Err(e) = self::check_readme(gctx, ws, pkg) {
				res.push(format!("{:}: Checking Readme file failed with: {:}", pkg.name(), e));
			}
			res
		});

		errors.iter().for_each(|s| error!("{:#?}", s));
		if !errors.is_empty() {
			anyhow::bail!("{} readme file(s) need to be updated (see above).", errors.len());
		}
	}

	let builds = packages.iter().map(|pkg| {
		check_metadata(pkg)?;

		let pkg_ws = Workspace::ephemeral(pkg.clone(), gctx, Some(ws.target_dir()), true)?;
		gctx.shell().status("Packing", pkg)?;
		match package(&pkg_ws, &opts) {
			Ok(mut rw_locks) if rw_locks.len() == 1 => {
				Ok((pkg_ws, rw_locks.pop().expect("we checked the count")))
			},
			Ok(rw_locks) => Err(anyhow::anyhow!(
				"Packing {} produced {} packages, expected one",
				pkg.name(),
				rw_locks.len()
			)),
			Err(e) => {
				cargo::display_error(&e, &mut gctx.shell());
				Err(anyhow::anyhow!("Failure packing {:}: {}", pkg.name(), e))
			},
		}
	});

	let (errors, successes): (Vec<_>, Vec<_>) = builds.partition(Result::is_err);

	for e in errors.iter().filter_map(|res| res.as_ref().err()) {
		error!("{:#?}", e);
	}
	if !errors.is_empty() {
		anyhow::bail!("Packing failed with {} errors (see above)", errors.len());
	};

	let build_mode = if build { CompileMode::Build } else { CompileMode::Check { test: false } };

	gctx.shell().status("Checking", "Packages")?;

	// Let's keep a reference to the already build packages and their unpacked
	// location, so they can be injected as dependencies to the packages build
	// later in the dependency graph. Through patching them in we make sure that
	// the packages can be build free of the workspace they orginated but together
	// with the other packages queued for release.
	let mut replaces = HashMap::new();

	for (pkg_ws, rw_lock) in successes.iter().filter_map(|e| e.as_ref().ok()) {
		gctx.shell()
			.status("Verfying", pkg_ws.current().expect("We've build localised workspaces. qed"))?;
		let ws = run_check_ephemeral(
			gctx,
			pkg_ws.current().unwrap(),
			rw_lock,
			&opts,
			build_mode,
			&replaces,
			&opts.targets,
		)?;
		let new_pkg = ws.current().expect("Each workspace is for a package!");
		replaces.insert(
			new_pkg.name().as_str().to_owned(),
			new_pkg
				.manifest_path()
				.parent()
				.expect("Folder exists")
				.to_str()
				.expect("Is stringifiable")
				.to_owned(),
		);
	}
	Ok(())
}
