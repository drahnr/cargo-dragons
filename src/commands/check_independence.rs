use std::str::FromStr;

use super::check::{run_check_ephemeral, run_check_inplace};
use cargo::{
	core::{package::Package, Workspace},
	ops::PackageOpts,
	util::command_prelude::CompileMode,
	GlobalContext,
};
use clap::builder::styling::{AnsiColor, Style};
use itertools::Itertools;

/// How the independence check will be performed
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
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
			Self::InPlace => String::from("Inplace"),
			Self::Ephemeral => String::from("Ephemeral"),
		}
	}
}

impl FromStr for IndependenceCtx {
	type Err = anyhow::Error;
	fn from_str(s: &str) -> Result<Self, Self::Err> {
		Ok(match s.to_lowercase().as_str() {
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

/// Creates a `Style` from a given color.
fn style_from_color(color: AnsiColor) -> Style {
	Style::new().fg_color(Some(color.into()))
}

pub fn independence_check(
	gctx: &GlobalContext,
	packages: Vec<Package>,
	opts: &PackageOpts<'_>,
	ws: Workspace<'_>,
	modes: Vec<CompileMode>,
	context: IndependenceCtx,
) -> Result<(), anyhow::Error> {
	let replace = Default::default();

	gctx.shell().status_with_color(
		"Processing",
		format!(
			"Running independence check using {} context for {} packages",
			context.to_string(),
			packages.len()
		),
		&style_from_color(AnsiColor::Magenta),
	)?;

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

			let n = feature_permutations.len();
			let name = package.name().as_str();
			gctx.shell().status_with_color(
				"Independence",
				format!(
				"{name} Checking compilation of these target permutations ({n}): {feature_permutations:?}"
			),
				&style_from_color(AnsiColor::Magenta),
			)?;

			let compile_mode_str = compile_mode_to_string(compile_mode).unwrap();
			let context_str = context.to_string();
			for features in feature_permutations.iter() {
				gctx.shell().status_with_color(
					format!("{context_str}/{compile_mode_str}"),
					format!(
						"{} {} with features {:?}",
						package.name(),
						package.version(),
						features
					),
					&style_from_color(AnsiColor::Cyan),
				)?;

				match context {
					IndependenceCtx::Ephemeral => {
						let tar_rw_lock = cargo::ops::package_one(&ws, package, opts)?;

						run_check_ephemeral(
							gctx,
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
						run_check_inplace(gctx, &ws, package, opts, *compile_mode, features)?;
					},
				};
			}
		}
	}

	gctx.shell().status_with_color(
		"Done",
		format!("Checking independence succeed for all {} packages", packages.len()),
		&style_from_color(AnsiColor::Magenta),
	)?;

	Ok(())
}
