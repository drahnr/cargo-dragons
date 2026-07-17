#[cfg(feature = "gen-readme")]
use crate::commands::readme;
use crate::util::strip_dev_dependencies_from_manifest;

use anyhow::Context;
use cargo::{
    GlobalContext,
    core::{
        Feature, SourceId, Workspace,
        compiler::{BuildConfig, CompileMode, UserIntent},
        dependency::DepKind,
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
    fs,
    io::{Seek, SeekFrom},
    path::{Path, PathBuf},
};
use tar::Archive;
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

fn user_intent(build_mode: CompileMode) -> anyhow::Result<UserIntent> {
    match build_mode {
        CompileMode::Build => Ok(UserIntent::Build),
        CompileMode::Test => Ok(UserIntent::Test),
        CompileMode::Check { test } => Ok(UserIntent::Check { test }),
        mode => anyhow::bail!("Unsupported compile mode: {mode:?}"),
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Replacement {
    name: String,
    path: PathBuf,
    patch_registries: Vec<String>,
}

fn patch_registries(package: &Package) -> anyhow::Result<Vec<String>> {
    match package.publish() {
        None => Ok(vec!["crates-io".to_owned()]),
        Some(registries) if registries.is_empty() => {
            anyhow::bail!("{} is marked as publish = false", package.name())
        }
        Some(registries) => Ok(registries.clone()),
    }
}

fn path_value(path: &Path) -> anyhow::Result<Value> {
    let path = path
        .to_str()
        .with_context(|| format!("Path is not valid UTF-8: {}", path.display()))?;
    Ok(Value::from(path).decorated(" ", " "))
}

fn write_patch_workspace_manifest(
    workspace_dir: &Path,
    package_dir: &Path,
    replacements: &HashMap<String, Replacement>,
) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(workspace_dir).with_context(|| {
        format!(
            "Could not create temporary patch workspace at {}",
            workspace_dir.display()
        )
    })?;
    let manifest_path = workspace_dir.join("Cargo.toml");
    let member = package_dir.to_str().with_context(|| {
        format!(
            "Could not create workspace member path for {}",
            package_dir.display()
        )
    })?;

    let mut document = DocumentMut::new();

    let mut members = Array::new();
    members.push(member);

    let mut workspace = Table::new();
    workspace["members"] = Item::Value(Value::Array(members));
    workspace["resolver"] = Item::Value(Value::from("2").decorated(" ", ""));
    document["workspace"] = Item::Table(workspace);

    for replacement in replacements.values() {
        for registry in &replacement.patch_registries {
            if document.get("patch").and_then(Item::as_table).is_none() {
                document["patch"] = Item::Table(Table::new());
            }

            let patch_table = document["patch"]
                .as_table_mut()
                .expect("patch should be a table");
            if !patch_table.contains_key(registry) {
                patch_table[registry] = Item::Table(Table::new());
            }

            let registry_table = patch_table[registry]
                .as_table_mut()
                .expect("patch registry should be a table");
            let mut patch = InlineTable::new();
            patch.insert("path", path_value(&replacement.path)?);
            registry_table[replacement.name.as_str()] = Item::Value(Value::InlineTable(patch));
        }
    }

    fs::write(&manifest_path, document.to_string().as_bytes()).with_context(|| {
        format!(
            "Could not write temporary patch workspace manifest at {}",
            manifest_path.display()
        )
    })?;
    Ok(manifest_path)
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
            filter: ops::CompileFilter::Default {
                required_features_filterable: true,
            },
            target_rustdoc_args: None,
            target_rustc_args: None,
            rustdoc_document_private_items: false,
            honor_rust_version: false.into(),
            target_rustc_crate_types: None,
        },
    )?;

    Ok(workspace)
}

pub(crate) fn run_check_ephemeral(
    gctx: &GlobalContext,
    package: &Package,
    tar: &FileLock,
    opts: &PackageOpts<'_>,
    build_mode: CompileMode,
    replace: &HashMap<String, Replacement>,
    features: &[String],
) -> anyhow::Result<Package> {
    let pkg = package;

    let mut tar_file = tar.file();
    tar_file.seek(SeekFrom::Start(0))?;
    let f = GzDecoder::new(tar_file);
    let dst = tar
        .parent()
        .join(format!("{}-{}", pkg.name(), pkg.version()));
    if dst.exists() {
        std::fs::remove_dir_all(&dst)?;
    }
    let mut archive = Archive::new(f);
    // We don't need to set the Modified Time, as it's not relevant to verification
    // and it errors on filesystems that don't support setting a modified timestamp
    archive.set_preserve_mtime(false);
    archive.unpack(dst.parent().expect("Expect Top level directory"))?;
    strip_dev_dependencies_from_manifest(&dst.join("Cargo.toml"))?;

    let id = SourceId::for_path(&dst)?;
    let mut src = PathSource::new(&dst, id, gctx);
    let new_pkg = src.root_package()?;

    let pkg_fingerprint = src.fingerprint(&new_pkg)?;
    let patch_workspace_manifest = write_patch_workspace_manifest(
        dst.parent()
            .context("Package directory should have a parent directory")?,
        &dst,
        replace,
    )?;
    let ws = Workspace::new(&patch_workspace_manifest, gctx)?;

    let rustc_args = if pkg
        .manifest()
        .unstable_features()
        .require(Feature::public_dependency())
        .is_ok()
    {
        // FIXME: Turn this on at some point in the future
        //Some(vec!["-D exported_private_dependencies".to_string()])
        Some(Vec::new())
    } else {
        None
    };

    let explicit = format!("{}@{}", package.name(), package.version());

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
            spec: ops::Packages::Packages(vec![explicit]),
            cli_features: CliFeatures::from_command_line(features, false, false)?,
            filter: ops::CompileFilter::Default {
                required_features_filterable: true,
            },
            target_rustdoc_args: None,
            target_rustc_args: rustc_args,
            rustdoc_document_private_items: false,
            honor_rust_version: None,
            target_rustc_crate_types: None,
        },
    )?;

    // Check that `build.rs` didn't modify any files in the `src` directory.
    let ws_fingerprint = src.fingerprint(&new_pkg)?;
    if pkg_fingerprint != ws_fingerprint {
        anyhow::bail!(
            "Source directory was modified by build.rs during cargo publish. \
             Build scripts should not modify anything outside of OUT_DIR.\n\
             {path:?}\n\n\
             To proceed despite this, pass the `--no-verify` flag.",
            path = ws_fingerprint
        );
    }

    Ok(new_pkg)
}

fn check_dependencies(package: &Package) -> Result<(), anyhow::Error> {
    let git_deps = Vec::from_iter(
        package
            .dependencies()
            .iter()
            .filter(|d| {
                d.kind() != DepKind::Development
                    && d.source_id().is_git()
                    && d.version_req() == &OptVersionReq::Any
            })
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
        _ => {}
    }
    match metadata.repository.as_deref() {
        Some("") => bad_fields.push("repository is empty"),
        None => bad_fields.push("repository is missing"),
        _ => {}
    }
    match (metadata.license.as_ref(), metadata.license_file.as_ref()) {
        (Some(s), None) | (None, Some(s)) if !s.is_empty() => {}
        (Some(_), Some(_)) => bad_fields.push("You can't have license AND license_file"),
        _ => bad_fields.push("Neither license nor license_file is provided"),
    }
    if metadata.keywords.len() > 5 {
        bad_fields.push("crates.io only allows up to 5 keywords")
    }

    if bad_fields.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "{}: Bad metadata: {}",
            package.name(),
            bad_fields.join("; ")
        )
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
        include_lockfile: false,
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
        anyhow::bail!(
            "Soft checkes failed with {} errors (see above)",
            errors.len()
        )
    }

    if check_readme {
        gctx.shell().status("Checking", "Readme files")?;
        let errors = packages.iter().fold(Vec::new(), |mut res, pkg| {
            if let Err(e) = self::check_readme(gctx, ws, pkg) {
                res.push(format!(
                    "{:}: Checking Readme file failed with: {:}",
                    pkg.name(),
                    e
                ));
            }
            res
        });

        errors.iter().for_each(|s| error!("{:#?}", s));
        if !errors.is_empty() {
            anyhow::bail!(
                "{} readme file(s) need to be updated (see above).",
                errors.len()
            );
        }
    }

    let build_mode = if build {
        CompileMode::Build
    } else {
        CompileMode::Check { test: false }
    };

    gctx.shell().status("Checking", "Packages")?;

    // Keep a reference to the already verified packages and their unpacked locations.
    // The loop is intentionally sequential: each package is verified with only the
    // packages that would already have been published earlier in the release order
    // available through the generated `[patch]` workspace manifest.
    let mut replaces = HashMap::new();

    for pkg in packages {
        check_metadata(pkg)?;
        gctx.shell().status("Packing", pkg)?;

        let package_ws = Workspace::ephemeral(pkg.clone(), gctx, Some(ws.target_dir()), true)?;
        let rw_lock = match package(&package_ws, &opts) {
            Ok(mut rw_locks) if rw_locks.len() == 1 => {
                rw_locks.pop().expect("we checked the count")
            }
            Ok(rw_locks) => anyhow::bail!(
                "Packing {} produced {} packages, expected one",
                pkg.name(),
                rw_locks.len()
            ),
            Err(e) => {
                cargo::display_error(&e, &mut gctx.shell());
                anyhow::bail!("Failure packing {:}: {}", pkg.name(), e)
            }
        };

        gctx.shell().status("Verfying", pkg)?;
        let new_pkg = run_check_ephemeral(
            gctx,
            pkg,
            &rw_lock,
            &opts,
            build_mode,
            &replaces,
            &opts.targets,
        )?;
        let patch_registries = patch_registries(&new_pkg)?;
        let path = new_pkg
            .manifest_path()
            .parent()
            .expect("Folder exists")
            .to_path_buf();
        replaces.insert(
            new_pkg.name().as_str().to_owned(),
            Replacement {
                name: new_pkg.name().as_str().to_owned(),
                path,
                patch_registries,
            },
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("cargo-dragons-{name}-{nanos}"))
    }

    #[test]
    fn temporary_manifest_stripping_removes_only_dev_dependencies() -> anyhow::Result<()> {
        let temp = unique_temp_dir("strip-dev-deps");
        fs::create_dir_all(&temp)?;
        let manifest = temp.join("Cargo.toml");
        fs::write(
            &manifest,
            r#"[package]
name = "strip-dev-deps"
version = "1.0.0"
edition = "2021"

[dependencies]
regular = "1"

[build-dependencies]
build-helper = "1"

[dev-dependencies]
dev-helper = "1"

[target.'cfg(unix)'.dependencies]
unix-regular = "1"

[target.'cfg(unix)'.dev-dependencies]
unix-dev-helper = "1"
"#,
        )?;

        strip_dev_dependencies_from_manifest(&manifest)?;

        let manifest = fs::read_to_string(&manifest)?;
        fs::remove_dir_all(&temp)?;
        assert!(manifest.contains("[dependencies]"));
        assert!(manifest.contains("regular"));
        assert!(manifest.contains("[build-dependencies]"));
        assert!(manifest.contains("build-helper"));
        assert!(manifest.contains("[target.'cfg(unix)'.dependencies]"));
        assert!(manifest.contains("unix-regular"));
        assert!(!manifest.contains("[dev-dependencies]"));
        assert!(!manifest.contains("dev-helper"));
        assert!(!manifest.contains("[target.'cfg(unix)'.dev-dependencies]"));
        assert!(!manifest.contains("unix-dev-helper"));
        Ok(())
    }

    #[test]
    fn generated_patch_workspace_applies_to_dev_dependencies() -> anyhow::Result<()> {
        let temp = unique_temp_dir("dev-dep-patch");
        let helper = temp.join("dev-helper");
        let package = temp.join("uses-dev-helper");
        let workspace = temp.clone();

        fs::create_dir_all(helper.join("src"))?;
        fs::write(
            helper.join("Cargo.toml"),
            "[package]\nname = \"dev-helper\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(helper.join("src/lib.rs"), "pub fn value() -> u8 { 42 }\n")?;

        fs::create_dir_all(package.join("src"))?;
        fs::create_dir_all(package.join("tests"))?;
        fs::write(
            package.join("Cargo.toml"),
            "[package]\nname = \"uses-dev-helper\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n[dev-dependencies]\ndev-helper = \"1\"\n",
        )?;
        fs::write(package.join("src/lib.rs"), "pub fn local() -> u8 { 1 }\n")?;
        fs::write(
            package.join("tests/dev_dep.rs"),
            "#[test]\nfn uses_patched_dev_dependency() { assert_eq!(dev_helper::value(), 42); }\n",
        )?;

        let replacements = HashMap::from([(
            "dev-helper".to_owned(),
            Replacement {
                name: "dev-helper".to_owned(),
                path: helper,
                patch_registries: vec!["crates-io".to_owned()],
            },
        )]);
        let manifest = write_patch_workspace_manifest(&workspace, &package, &replacements)?;

        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let output = Command::new(cargo)
            .arg("test")
            .arg("--manifest-path")
            .arg(&manifest)
            .arg("--package")
            .arg("uses-dev-helper")
            .arg("--test")
            .arg("dev_dep")
            .arg("--offline")
            .output()?;

        fs::remove_dir_all(&temp)?;
        assert!(
            output.status.success(),
            "cargo test failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn generated_patch_workspace_applies_to_unpublished_previous_versions() -> anyhow::Result<()> {
        let temp = unique_temp_dir("previous-version-patch");
        let previous = temp.join("versioned-helper-0.1.0");
        let package = temp.join("versioned-helper-0.2.0");
        let workspace = temp.clone();

        fs::create_dir_all(previous.join("src"))?;
        fs::write(
            previous.join("Cargo.toml"),
            "[package]\nname = \"versioned-helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(
            previous.join("src/lib.rs"),
            "pub fn previous() -> u8 { 1 }\n",
        )?;

        fs::create_dir_all(package.join("src"))?;
        fs::write(
            package.join("Cargo.toml"),
            "[package]\nname = \"versioned-helper\"\nversion = \"0.2.0\"\nedition = \"2021\"\n\n[dependencies]\nprevious-versioned-helper = { package = \"versioned-helper\", version = \"0.1\" }\n",
        )?;
        fs::write(
            package.join("src/lib.rs"),
            "pub fn current() -> u8 { previous_versioned_helper::previous() + 1 }\n",
        )?;

        let replacements = HashMap::from([(
            "versioned-helper".to_owned(),
            Replacement {
                name: "versioned-helper".to_owned(),
                path: previous,
                patch_registries: vec!["crates-io".to_owned()],
            },
        )]);
        let manifest = write_patch_workspace_manifest(&workspace, &package, &replacements)?;

        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let output = Command::new(cargo)
            .arg("check")
            .arg("--manifest-path")
            .arg(&manifest)
            .arg("--package")
            .arg("versioned-helper@0.2.0")
            .arg("--offline")
            .output()?;

        fs::remove_dir_all(&temp)?;
        assert!(
            output.status.success(),
            "cargo check failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }
}
