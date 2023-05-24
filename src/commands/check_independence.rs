use super::{
	super::cli::IndependenceCtx,
	check::{run_check_ephemeral, run_check_inplace},
};
use cargo::{
	core::{package::Package, Workspace},
	ops::PackageOpts,
	util::command_prelude::CompileMode,
};

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

	log::info!("Running independence check using {} context", context.to_string());

	for package in packages.iter() {
		for compile_mode in modes.iter() {
			log::info!(
				"Checking independence for package {} with {} mode and {} context",
				package,
				compile_mode_to_string(compile_mode).unwrap(),
				context.to_string()
			);

			match context {
				IndependenceCtx::Ephemeral => {
					let tar_rw_lock = cargo::ops::package_one(&ws, package, opts)?
						.expect("Not listing, hence result is always `Some(_)`. qed");

					run_check_ephemeral(&ws, package, &tar_rw_lock, opts, *compile_mode, &replace)?;
				},
				IndependenceCtx::InPlace => {
					run_check_inplace(&ws, package, opts, *compile_mode)?;
				},
			};
		}
	}
	log::info!("Checking independence succeed for all {} packages", packages.len());

	Ok(())
}
