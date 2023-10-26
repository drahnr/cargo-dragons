use std::str::FromStr;

use super::check::{run_check_ephemeral, run_check_inplace};
use cargo::{
	core::{package::Package, Workspace},
	ops::PackageOpts,
	util::command_prelude::CompileMode,
};
use itertools::Itertools;

/// How the independence check will be performed
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndependenceCtx {
	/// Compile 'in place', stay within the tree, but only compile the selected packages.
	///
	/// Commonly required when there are path-only or git based dependencies.
	#[default]
	InPlace,
	/// Use a ephemeral workspace, outside of the current workspace.
	Ephemeral,
}

impl ToString for IndependenceCtx {
	fn to_string(&self) -> String {
		match self {
			Self::InPlace => String::from("inplace"),
			Self::Ephemeral => String::from("ephemeral"),
		}
	}
}

impl FromStr for IndependenceCtx {
	type Err = anyhow::Error;
	fn from_str(s: &str) -> Result<Self, Self::Err> {
		Ok(match s {
			"in-place" | "inplace" | "in_place" => Self::InPlace,
			"ephemeral" => Self::Ephemeral,
			c => anyhow::bail!("Unknown context: {}", c),
		})
	}
}

/// What to check for in the independence check
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum IndependenceMode {
	/// Run `cargo check`
	Check,
	/// Run `cargo build`
	Build,
	/// Run `cargo test`
	Test,
}

impl FromStr for IndependenceMode {
	type Err = anyhow::Error;
	fn from_str(s: &str) -> Result<Self, Self::Err> {
		Ok(match s {
			"check" => Self::Check,
			"build" => Self::Build,
			"test" => Self::Test,
			c => anyhow::bail!("Unknown check type: {}", c),
		})
	}
}

fn compile_mode_to_string(src: &CompileMode) -> Result<&'static str, anyhow::Error> {
	Ok(match src {
		CompileMode::Build => "build",
		CompileMode::Test => "test",
		CompileMode::Check { test: false } => "check",
		_ => anyhow::bail!("Unknown or unsupported mode : {:?}", src),
	})
}

pub fn independence_check(
	packages: Vec<Package>,
	opts: &PackageOpts<'_>,
	ws: Workspace<'_>,
	modes: Vec<CompileMode>,
	context: IndependenceCtx,
) -> Result<(), anyhow::Error> {
	let replace = Default::default();

	println!(
		"Running independence check using {} context for {} packages",
		context.to_string(),
		packages.len()
	);

	for package in packages.iter() {
		for compile_mode in modes.iter() {
			// Get all unique feature combinations to ensure their compilation.
			let feature_permutations = Vec::from_iter(
				package
					.summary()
					.features()
					.keys()
					.powerset()
					.map(|v| Vec::from_iter(v.iter().map(|v| v.to_string()))),
			);
			println!("Checking compilation of these target permutations: {feature_permutations:?}");

			for features in feature_permutations.iter() {
				println!(
					"{}: Running {} independence for package {}, with features {:?}",
					context.to_string(),
					compile_mode_to_string(compile_mode).unwrap(),
					package.name(),
					features
				);
				match context {
					IndependenceCtx::Ephemeral => {
						let tar_rw_lock = cargo::ops::package_one(&ws, package, opts)?
							.expect("Not listing, hence result is always `Some(_)`. qed");

						run_check_ephemeral(
							&ws,
							package,
							&tar_rw_lock,
							opts,
							*compile_mode,
							&replace,
							features,
						)?;
					},
					IndependenceCtx::InPlace => {
						run_check_inplace(&ws, package, opts, *compile_mode, features)?;
					},
				};
			}
		}
	}
	println!("Checking independence succeed for all {} packages", packages.len());

	Ok(())
}
