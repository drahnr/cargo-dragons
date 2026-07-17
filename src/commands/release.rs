use crate::{commands::add_owner, util::strip_dev_dependencies_from_manifest};
use anyhow::Context;
use cargo::{
    GlobalContext,
    core::{SourceId, Workspace, package::Package, resolver::features::CliFeatures},
    ops::{self, PublishOpts, publish},
    sources::PathSource,
};
use cargo_credential::Secret;

use std::{thread, time::Duration};

fn drop_dev_dependencies_from_worktree(
    gctx: &GlobalContext,
    packages: &[Package],
) -> anyhow::Result<()> {
    gctx.shell()
        .status("Dropping dev-dependencies", "worktree manifests")?;
    for package in packages {
        gctx.shell().status("Dropping dev-dependencies", package)?;
        strip_dev_dependencies_from_manifest(package.manifest_path())?;
    }
    Ok(())
}

fn reload_package_from_worktree(
    gctx: &GlobalContext,
    package: &Package,
) -> anyhow::Result<Package> {
    let package_dir = package
        .manifest_path()
        .parent()
        .context("Package manifest path should have a parent directory")?;
    let source_id = SourceId::for_path(package_dir)?;
    let mut source = PathSource::new(package_dir, source_id, gctx);
    let reloaded = source.root_package().with_context(|| {
        format!(
            "Could not reload package {} from {} after dropping dev-dependencies",
            package.name(),
            package.manifest_path().display()
        )
    })?;

    if reloaded.name() != package.name() || reloaded.version() != package.version() {
        anyhow::bail!(
            "Reloaded package changed identity from {} {} to {} {}",
            package.name(),
            package.version(),
            reloaded.name(),
            reloaded.version()
        );
    }

    Ok(reloaded)
}

fn reload_package_from_workspace(
    gctx: &GlobalContext,
    ws: &Workspace<'_>,
    package: &Package,
) -> anyhow::Result<Package> {
    let manifest_path = package.manifest_path();
    if let Some(member) = ws.members().find(|member| {
        member.manifest_path() == manifest_path
            && member.name() == package.name()
            && member.version() == package.version()
    }) {
        return Ok(member.clone());
    }

    reload_package_from_worktree(gctx, package)
}

fn reload_packages_from_worktree(
    gctx: &GlobalContext,
    ws: &Workspace<'_>,
    packages: &[Package],
) -> anyhow::Result<Vec<Package>> {
    packages
        .iter()
        .map(|package| reload_package_from_workspace(gctx, ws, package))
        .collect()
}

pub fn release(
    gctx: &GlobalContext,
    packages: Vec<Package>,
    ws: Workspace<'_>,
    dry_run: bool,
    token: Option<Secret<String>>,
    owner: Option<String>,
) -> Result<(), anyhow::Error> {
    let cli_features = CliFeatures {
        features: Default::default(),
        all_features: false,
        uses_default_features: true,
    };

    if dry_run {
        let opts = ops::PackageOpts {
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
            cli_features,
            keep_going: false,
            reg_or_index: None,
            dry_run: true,
        };

        // Keep the ephemeral packaging configuration above in sync with real package
        // options, but do not call Cargo's package/publish dry-run here: it performs
        // registry availability checks after path dependencies are stripped, which
        // fails for unpublished workspace crates released together.
        drop(opts);

        gctx.shell()
            .status("Dry run packaging (🏜️):", packages.len())?;
        for pkg in packages {
            gctx.shell().status("Would package (🏜️)", &pkg)?;
        }
        return Ok(());
    }

    drop_dev_dependencies_from_worktree(gctx, &packages)?;
    let reloaded_ws = Workspace::new(ws.root_manifest(), gctx)?;
    let packages = reload_packages_from_worktree(gctx, &reloaded_ws, &packages)?;

    let opts = PublishOpts {
        gctx,
        verify: false,
        token: token.clone(),
        dry_run: false,
        allow_dirty: true,
        jobs: None,
        to_publish: ops::Packages::Default,
        targets: Default::default(),
        cli_features,
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

    gctx.shell()
        .status("Publishing (🚂🚃🚃):", packages.len())?;
    for (idx, pkg) in packages.iter().enumerate() {
        if idx > 0 && delay > 0 {
            gctx.shell().status(
                "Waiting",
                "published 30 crates – API limits require us to wait in between.",
            )?;
            thread::sleep(Duration::from_secs(delay));
        }

        let pkg_ws = Workspace::ephemeral(pkg.clone(), gctx, Some(ws.target_dir()), true)?;
        gctx.shell().status("Publishing (🚂🚃🚃)", pkg)?;
        publish(&pkg_ws, &opts)?;
        if let Some(ref o) = owner {
            add_owner(gctx, pkg, o.clone(), token.clone())?;
        }
    }
    Ok(())
}
