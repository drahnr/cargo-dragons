use crate::commands::add_owner;
use cargo::{
	core::{package::Package, resolver::features::CliFeatures, Workspace},
	ops::{self, publish, PublishOpts},
	GlobalContext,
};
use cargo_credential::Secret;

use std::{thread, time::Duration};

pub fn release(
	gctx: &GlobalContext,
	packages: Vec<Package>,
	ws: Workspace<'_>,
	dry_run: bool,
	token: Option<Secret<String>>,
	owner: Option<String>,
) -> Result<(), anyhow::Error> {
	let opts = PublishOpts {
		gctx,
		verify: false,
		token: token.clone(),
		dry_run,
		allow_dirty: true,
		jobs: None,
		to_publish: ops::Packages::Default,
		targets: Default::default(),
		cli_features: CliFeatures {
			features: Default::default(),
			all_features: false,
			uses_default_features: true,
		},
		keep_going: false,
		reg_or_index: None,
	};
	let delay = {
		if packages.len() > 29 {
			// more than 30, delay so we do not publish more than 30 in 10min.
			// 20 seconds per publish so wait 21 to ensure at least a package is done
			21
		} else {
			// below the limit we just burst them out.
			0
		}
	};

	gctx.shell().status("Publishing", "Packages")?;
	for (idx, pkg) in packages.iter().enumerate() {
		if idx > 0 && delay > 0 {
			gctx.shell().status(
				"Waiting",
				"published 30 crates – API limits require us to wait in between.",
			)?;
			thread::sleep(Duration::from_secs(delay));
		}

		let pkg_ws = Workspace::ephemeral(pkg.clone(), gctx, Some(ws.target_dir()), true)?;
		gctx.shell().status("Publishing", pkg)?;
		publish(&pkg_ws, &opts)?;
		if let Some(ref o) = owner {
			add_owner(gctx, pkg, o.clone(), token.clone())?;
		}
	}
	Ok(())
}
