use anyhow::Context;
use cargo::{
    GlobalContext,
    core::{Workspace, dependency::DepKind, package::Package},
    sources::PathSource,
};
use colorize::AnsiColor;
use git2::Repository;
use log::{trace, warn};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};
use toml_edit::{DocumentMut, InlineTable, Item, Table, Value};

use crate::cli::PackageSelectOptions;

pub fn changed_packages(
    gctx: &GlobalContext,
    ws: &Workspace,
    reference: &str,
) -> Result<HashSet<Package>, anyhow::Error> {
    gctx.shell()
        .status("Calculating", format!("git diff since {:}", reference))
        .expect("Writing to Shell doesn't fail");

    let path = ws.root();
    let repo = Repository::open(path).context("Workspace isn't a git repo")?;
    let current_head = repo
        .head()
        .and_then(|b| b.peel_to_commit())
        .and_then(|c| c.tree())
        .context("Could not determine current git HEAD")?;
    let main = repo
        .resolve_reference_from_short_name(reference)
        .and_then(|d| d.peel_to_commit())
        .and_then(|c| c.tree())
        .context("Reference not found in git repository")?;

    let diff = repo
        .diff_tree_to_tree(Some(&current_head), Some(&main), None)
        .context("Diffing failed")?;

    let files = Vec::from_iter(
        diff.deltas()
            .filter_map(|d| d.new_file().path())
            .filter_map(|d| if d.is_file() { d.parent() } else { Some(d) })
            .map(|l| path.join(l)),
    );

    trace!("Files changed since: {:#?}", files);

    let mut packages = HashSet::new();

    for m in members_deep(gctx, ws) {
        let root = m.root();
        for f in files.iter() {
            if f.starts_with(root) {
                packages.insert(m);
                break;
            }
        }
    }

    Ok(packages)
}

/// Remove dev-dependency sections from a manifest.
pub fn strip_dev_dependencies_from_manifest(manifest_path: &Path) -> anyhow::Result<()> {
    let manifest = fs::read_to_string(manifest_path)
        .with_context(|| format!("Could not read manifest at {}", manifest_path.display()))?;
    let mut document = manifest
        .parse::<DocumentMut>()
        .with_context(|| format!("Could not parse manifest at {}", manifest_path.display()))?;

    let mut changed = false;
    let root = document.as_table_mut();
    changed |= root.remove("dev-dependencies").is_some();
    changed |= root.remove("dev_dependencies").is_some();

    if let Some(targets) = root.get_mut("target").and_then(Item::as_table_mut) {
        for (_, target) in targets.iter_mut() {
            if let Some(target) = target.as_table_mut() {
                changed |= target.remove("dev-dependencies").is_some();
                changed |= target.remove("dev_dependencies").is_some();
            }
        }
    }

    if changed {
        fs::write(manifest_path, document.to_string())
            .with_context(|| format!("Could not write manifest at {}", manifest_path.display()))?;
    }

    Ok(())
}

pub fn members_deep(gctx: &GlobalContext, ws: &'_ Workspace) -> Vec<Package> {
    let mut total_list = Vec::new();
    for m in ws.members() {
        total_list.push(m.clone());
        for dep in m.dependencies() {
            if dep.kind() == DepKind::Development {
                trace!(
                    "Ignoring dev-dependency while discovering release candidates: {} -> {}",
                    m.name(),
                    dep.package_name()
                );
                continue;
            }

            let source = dep.source_id();
            if source.is_path() {
                let dst = source
                    .url()
                    .to_file_path()
                    .expect("It was just checked before. qed");
                let mut src = PathSource::new(&dst, source, gctx);
                let pkg = src.root_package().expect("Path must have a package");
                if !ws.is_member(&pkg) {
                    total_list.push(pkg);
                }
            }
        }
    }
    total_list
}

fn get_type_of<T>(_: &T) -> String {
    std::any::type_name::<T>().to_owned()
}
/// Run f on every package's manifest, write the doc. Fail on first error
pub fn edit_each<'a, I, F, R>(iter: I, f: F) -> Result<Vec<R>, anyhow::Error>
where
    F: Fn(&'a Package, &mut DocumentMut) -> Result<R, anyhow::Error>,
    I: Iterator<Item = &'a Package>,
{
    let mut results = Vec::new();
    for pkg in iter {
        let manifest_path = pkg.manifest_path();
        let content = fs::read_to_string(manifest_path)?;
        let mut doc: DocumentMut = content.parse()?;
        results.push(f(pkg, &mut doc)?);
        fs::write(manifest_path, doc.to_string())?;
    }
    Ok(results)
}

/// Wrap each the different dependency as a mutable item
pub enum DependencyEntry<'a> {
    Table(&'a mut Table),
    Inline(&'a mut InlineTable),
}

#[derive(Debug, PartialEq, Eq)]
/// The action (should be) taken on the dependency entry
pub enum DependencyAction {
    /// Ignored, we didn't touch
    Untouched,
    /// Entry was changed, needs to be saved
    Mutated,
    /// Remove this entry and save the manifest
    Remove,
}

#[derive(Debug, PartialEq, Eq, Clone)]
/// Which Dependency Section a dependency belongs to
pub enum DependencySection {
    /// Just a regular `dependency`
    Regular,
    /// A `dev-`dependency
    Dev,
    /// A build dependency
    Build,
}

impl DependencySection {
    fn key(&self) -> &'static str {
        match self {
            DependencySection::Regular => "dependencies",
            DependencySection::Dev => "dev-dependencies",
            DependencySection::Build => "build-dependencies",
        }
    }
}

/// Iterate through the dependency sections of root, find each
/// dependency entry, that is a subsection and hand it and its name
/// to f. Return the counter of how many times f returned true.
pub fn edit_each_dep<F>(root: &mut Table, f: F) -> u32
where
    F: Fn(String, Option<String>, DependencyEntry, DependencySection) -> DependencyAction,
{
    let mut counter = 0;
    let mut removed = Vec::new();
    for case in [
        DependencySection::Regular,
        DependencySection::Dev,
        DependencySection::Build,
    ] {
        let k = case.key();
        if let Some(Item::Table(t)) = root.get_mut(k) {
            let keys = Vec::from_iter(t.iter().filter_map(|(key, v)| {
                if v.is_table() || v.is_inline_table() {
                    Some(key.to_owned())
                } else {
                    None
                }
            }));
            for key in keys {
                let (name, action) = match t.get_mut(&key) {
                    Some(Item::Value(Value::InlineTable(info))) => {
                        let (name, alias) = info
                            .get("package")
                            .map(|name| {
                                (
                                    name.as_str()
                                        .expect("Package is always a valid UTF-8. qed")
                                        .to_owned(),
                                    Some(key.clone()),
                                )
                            })
                            .unwrap_or_else(|| (key.clone(), None));
                        (
                            name.clone(),
                            f(name, alias, DependencyEntry::Inline(info), case.clone()),
                        )
                    }
                    Some(Item::Table(info)) => {
                        let (name, alias) = info
                            .get("package")
                            .map(|name| {
                                (
                                    name.as_str()
                                        .expect("Package is always a valid UTF-8. qed")
                                        .to_owned(),
                                    Some(key.clone()),
                                )
                            })
                            .unwrap_or_else(|| (key.clone(), None));
                        (
                            name.clone(),
                            f(name, alias, DependencyEntry::Table(info), case.clone()),
                        )
                    }
                    None => continue,
                    info => {
                        warn!(
                            "Unsupported dependency format for {}. Format must be InlinedTable/Table, not {}",
                            key,
                            get_type_of(&info)
                        );
                        (key.clone(), DependencyAction::Untouched)
                    }
                };

                match action {
                    DependencyAction::Remove => {
                        t.remove(&name);
                        removed.push(name);
                    }
                    DependencyAction::Untouched => { /* nop */ }
                    _ => {
                        counter += 1;
                    }
                }
            }
        }
    }

    if !removed.is_empty()
        && let Some(Item::Table(features)) = root.get_mut("features")
    {
        let keys = Vec::from_iter(features.iter().map(|(k, _v)| k.to_owned()));
        for feat in keys {
            if let Some(Item::Value(Value::Array(deps))) = features.get_mut(&feat) {
                let mut to_remove = Vec::new();
                for (idx, dep) in deps.iter().enumerate() {
                    if let Value::String(s) = dep
                        && let Some(s) = s.value().trim().split('/').next()
                        && removed.contains(&s.to_owned())
                    {
                        to_remove.push(idx);
                    }
                }
                if !to_remove.is_empty() {
                    // remove starting from the end:
                    to_remove.reverse();
                    for idx in to_remove {
                        deps.remove(idx);
                    }
                }
            }
        }
    }
    counter
}

/// How empty packages are handled
#[derive(clap::ValueEnum, Debug, Clone, Copy)]
pub(crate) enum EmptyPackage {
    // Finding an Empty Package is not a failure.
    Ignore,
    // Finding an Empty Package is a failure.
    Fail,
}

/// Convert a `bool` value to an `EmptyPackage` type
pub(crate) fn empty_package_bool_to_action(empty_package_is_failure: bool) -> EmptyPackage {
    if empty_package_is_failure {
        return EmptyPackage::Fail;
    }
    EmptyPackage::Ignore
}

pub(crate) fn handle_empty_package_is_failures_with_available<T>(
    packages: &[T],
    empty_package_is_failure: bool,
    available_packages: Option<&[String]>,
) -> anyhow::Result<bool> {
    if packages.is_empty() {
        let empty_package_action = empty_package_bool_to_action(empty_package_is_failure);
        match empty_package_action {
            EmptyPackage::Ignore => {
                println!("No packages selected. All good. Exiting.");
                if let Some(available_packages) = available_packages {
                    println!(
                        "\nAvailable packages (showing up to {MAX_AVAILABLE_PACKAGE_SUGGESTIONS}):\n{}\n\nHow to fix: bump the version of the crate you want to release if it is already published, or use `--packages <name>` to target one of the packages above. If a package is skipped because it has `publish = false`, pass `--ignore-publish`.",
                        format_available_packages(None, available_packages)
                    );
                }
                return Ok(true);
            }
            EmptyPackage::Fail => {
                anyhow::bail!("No packages matching criteria. Exiting");
            }
        }
    }
    Ok(false)
}

fn is_publishable(p: &Package, ignore_publish: bool) -> bool {
    // If publish is set to false or any registry, it is ignored by default
    // unless overriden.
    let value = ignore_publish || p.publish().is_none();

    trace!("{:}.publish={}", p.name(), value);
    value
}

const MAX_AVAILABLE_PACKAGE_SUGGESTIONS: usize = 10;

pub(crate) fn available_package_names(gctx: &GlobalContext, ws: &Workspace<'_>) -> Vec<String> {
    let mut available_packages = Vec::from_iter(
        members_deep(gctx, ws)
            .iter()
            .map(|p| p.name().as_str().to_owned()),
    );
    available_packages.sort();
    available_packages.dedup();
    available_packages
}

fn bold_package_name(name: &str) -> String {
    name.to_owned().bold()
}

fn ranked_available_packages<'a>(
    selector: Option<&str>,
    package_names: &'a [String],
) -> Vec<&'a String> {
    let Some(selector) = selector else {
        return package_names.iter().collect();
    };
    let selector = selector.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
    let mut ranked = package_names.iter().collect::<Vec<_>>();
    ranked.sort_by_key(|name| (levenshtein(selector, name), name.as_str()));
    ranked
}

fn format_available_packages(selector: Option<&str>, package_names: &[String]) -> String {
    let ranked = ranked_available_packages(selector, package_names);
    let mut lines = ranked
        .iter()
        .take(MAX_AVAILABLE_PACKAGE_SUGGESTIONS)
        .map(|name| format!("  - {}", bold_package_name(name)))
        .collect::<Vec<_>>();

    if ranked.len() > MAX_AVAILABLE_PACKAGE_SUGGESTIONS {
        lines.push(format!(
            "  ... and {} more",
            ranked.len() - MAX_AVAILABLE_PACKAGE_SUGGESTIONS
        ));
    }

    lines.join("\n")
}

fn closest_package_suggestion<'a>(selector: &str, package_names: &'a [String]) -> Option<&'a str> {
    let selector = selector.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
    let (name, distance) = package_names
        .iter()
        .map(|name| (name.as_str(), levenshtein(selector, name)))
        .min_by_key(|(_, distance)| *distance)?;
    let max_distance = std::cmp::max(2, selector.chars().count() / 3);
    (distance <= max_distance).then_some(name)
}

fn levenshtein(left: &str, right: &str) -> usize {
    let right_chars = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right_chars.len()).collect::<Vec<_>>();
    let mut current = vec![0; right_chars.len() + 1];

    for (left_index, left_char) in left.chars().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_char) in right_chars.iter().enumerate() {
            let substitution_cost = usize::from(left_char != *right_char);
            current[right_index + 1] = std::cmp::min(
                std::cmp::min(current[right_index] + 1, previous[right_index + 1] + 1),
                previous[right_index] + substitution_cost,
            );
        }
        std::mem::swap(&mut previous, &mut current);
    }

    previous[right_chars.len()]
}

pub(crate) fn make_pkg_predicate(
    gctx: &GlobalContext,
    ws: &Workspace<'_>,
    args: PackageSelectOptions,
) -> Result<impl Fn(&Package) -> bool + 'static, anyhow::Error> {
    let PackageSelectOptions {
        packages,
        skip,
        ignore_pre_version,
        ignore_publish,
        changed_since,
        include_pre_deps,
        list_packages: _,
    } = args;

    let members = members_deep(gctx, ws);
    let available_packages = available_package_names(gctx, ws);

    if !packages.is_empty() {
        if !skip.is_empty() || !ignore_pre_version.is_empty() {
            anyhow::bail!(
                "-p/--packages is mutually exclusive to using -s/--skip and -i/--ignore-version-pre"
            );
        }
        if changed_since.is_some() {
            anyhow::bail!("-p/--packages is mutually exclusive to using -c/--changed-since");
        }
    }

    for package_selector in &packages {
        if !available_packages
            .iter()
            .any(|name| package_selector.is_match(name))
        {
            anyhow::bail!(
                "Package selector `{}` did not match any packages.\n\nAvailable packages (closest matches first, showing up to {MAX_AVAILABLE_PACKAGE_SUGGESTIONS}):\n{}{}",
                package_selector.as_str(),
                format_available_packages(Some(package_selector.as_str()), &available_packages),
                closest_package_suggestion(package_selector.as_str(), &available_packages)
                    .map(|name| {
                        format!(
                            "\n\nDid you mean `{}`?\nHow to fix: run again with `--packages {name}`.",
                            bold_package_name(name)
                        )
                    })
                    .unwrap_or_else(|| "\n\nHow to fix: run again with one of the package names listed above.".to_owned())
            );
        }
    }

    let changed = if let Some(changed_since) = &changed_since {
        if !skip.is_empty() || !ignore_pre_version.is_empty() {
            anyhow::bail!(
                "-c/--changed-since is mutually exclusive to using -s/--skip and -i/--ignore-version-pre"
            );
        }
        Some(crate::util::changed_packages(gctx, ws, changed_since)?)
    } else {
        None
    };

    let base_selected = move |p: &Package| {
        if !is_publishable(p, ignore_publish) {
            return false;
        }

        if let Some(changed) = &changed {
            return changed.contains(p);
        }

        if !packages.is_empty() {
            trace!("going for matching against {:?}", packages);
            let name = p.name();
            return packages.iter().any(|r| r.is_match(&name));
        }

        if !skip.is_empty() || !ignore_pre_version.is_empty() {
            let name = p.name();
            if skip.iter().any(|r| r.is_match(&name)) {
                return false;
            }
            if !p.version().pre.is_empty()
                && ignore_pre_version.contains(&p.version().pre.as_str().to_owned())
            {
                return false;
            }
        }

        true
    };

    let mut selected_pre_dependencies = HashSet::new();
    if include_pre_deps {
        let members_by_name = HashMap::<String, &Package>::from_iter(
            members.iter().map(|p| (p.name().as_str().to_owned(), p)),
        );
        let mut stack = Vec::from_iter(
            members
                .iter()
                .filter(|p| base_selected(p))
                .map(|p| p.name().as_str().to_owned()),
        );

        while let Some(name) = stack.pop() {
            let Some(package) = members_by_name.get(&name) else {
                continue;
            };

            for dep in package.dependencies() {
                let dep_name = dep.package_name().as_str();
                let Some(dep_package) = members_by_name.get(dep_name) else {
                    continue;
                };

                if is_publishable(dep_package, ignore_publish)
                    && !dep_package.version().pre.is_empty()
                    && selected_pre_dependencies.insert(dep_name.to_owned())
                {
                    stack.push(dep_name.to_owned());
                }
            }
        }
    }

    Ok(move |p: &Package| {
        if !is_publishable(p, ignore_publish) {
            return false;
        }

        base_selected(p) || selected_pre_dependencies.contains(p.name().as_str())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_package_listing_is_bold_and_capped() {
        let packages = (0..12)
            .map(|index| format!("crate-{index}"))
            .collect::<Vec<_>>();

        let listing = format_available_packages(None, &packages);

        assert_eq!(listing.matches("  - ").count(), 10);
        assert!(listing.contains("\u{1b}[1mcrate-0"));
        assert!(listing.contains("... and 2 more"));
    }
}
