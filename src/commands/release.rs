use crate::{
    registry::{PublicationRegistry, RegistryTokens},
    util::devdeps::DevDependencyRestoreSession,
};

use super::add_owner::add_owner_for_registry;
use anyhow::Context;
use cargo::{
    GlobalContext,
    core::{SourceId, Workspace, package::Package, resolver::features::CliFeatures},
    ops::{self, PublishOpts, publish},
    sources::PathSource,
};

use std::{thread, time::Duration};

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
    anyhow::Result::<Vec<_>>::from_iter(
        packages
            .iter()
            .map(|package| reload_package_from_workspace(gctx, ws, package)),
    )
}

pub fn release(
    gctx: &GlobalContext,
    packages: Vec<Package>,
    ws: Workspace<'_>,
    dry_run: bool,
    tokens: RegistryTokens,
    owner: Option<String>,
) -> Result<(), anyhow::Error> {
    let registries = anyhow::Result::<Vec<_>>::from_iter(
        packages
            .iter()
            .map(|package| PublicationRegistry::for_package(gctx, package)),
    )?;

    let cli_features = CliFeatures {
        features: Default::default(),
        all_features: false,
        uses_default_features: true,
    };

    if dry_run {
        gctx.shell()
            .status("Dry-run packaging (🏜️):", packages.len())?;
        for (pkg, registry) in packages.iter().zip(&registries) {
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
                cli_features: cli_features.clone(),
                keep_going: false,
                reg_or_index: Some(registry.reg_or_index()),
                dry_run: true,
            };

            // Keep the ephemeral packaging configuration above in sync with real package
            // options, but do not call Cargo's package/publish dry-run here: it performs
            // registry availability checks after path dependencies are stripped, which
            // fails for unpublished workspace crates released together.
            drop(opts);

            gctx.shell().status("Would package (🏜️)", pkg)?;
        }
        return Ok(());
    }

    let target_dir = ws.target_dir();
    let mut dev_dependency_restore =
        DevDependencyRestoreSession::new(target_dir.as_path_unlocked());
    gctx.shell()
        .status("Dropping dev-dependencies", "worktree manifests")?;
    for package in &packages {
        let _ = gctx.shell().status("Dropping dev-dependencies", package);
        dev_dependency_restore.strip_manifest(package.name().as_str(), package.manifest_path())?;
    }

    let release_result = (|| -> Result<(), anyhow::Error> {
        let reloaded_ws = Workspace::new(ws.root_manifest(), gctx)?;
        let packages = reload_packages_from_worktree(gctx, &reloaded_ws, &packages)?;

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
        for (idx, (pkg, registry)) in packages.iter().zip(&registries).enumerate() {
            if idx > 0 && delay > 0 {
                gctx.shell().status(
                    "Waiting",
                    "published 30 crates – API limits require us to wait in between.",
                )?;
                thread::sleep(Duration::from_secs(delay));
            }

            let pkg_ws = Workspace::ephemeral(pkg.clone(), gctx, Some(ws.target_dir()), true)?;
            let opts = PublishOpts {
                gctx,
                verify: false,
                token: tokens.for_registry(registry).cloned(),
                dry_run: false,
                allow_dirty: true,
                jobs: None,
                to_publish: ops::Packages::Packages(vec![pkg.name().as_str().to_owned()]),
                targets: Default::default(),
                cli_features: cli_features.clone(),
                keep_going: false,
                reg_or_index: Some(registry.reg_or_index()),
            };
            gctx.shell().status("Publishing (🚂🚃🚃)", pkg)?;
            publish(&pkg_ws, &opts)?;
            if let Some(ref o) = owner {
                add_owner_for_registry(
                    gctx,
                    pkg,
                    o.clone(),
                    tokens.for_registry(registry).cloned(),
                    registry,
                )?;
            }
        }
        Ok(())
    })();
    let _ = gctx
        .shell()
        .status("Restoring dev-dependencies", "worktree manifests");
    let restore_result = dev_dependency_restore.restore_all();

    release_result?;
    restore_result.context("Release succeeded, but restoring dev-dependencies failed")
}
