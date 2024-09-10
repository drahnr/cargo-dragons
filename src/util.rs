use anyhow::Context;
use cargo::{
	core::{package::Package, Workspace},
	sources::PathSource,
	GlobalContext,
};
use git2::Repository;
use log::{trace, warn};
use std::{collections::HashSet, fs};
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

// Find all members of the workspace, into the total depth
pub fn members_deep(gctx: &GlobalContext, ws: &'_ Workspace) -> Vec<Package> {
	let mut total_list = Vec::new();
	for m in ws.members() {
		total_list.push(m.clone());
		for dep in m.dependencies() {
			let source = dep.source_id();
			if source.is_path() {
				let dst = source.url().to_file_path().expect("It was just checked before. qed");
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
	for case in [DependencySection::Regular, DependencySection::Dev, DependencySection::Build] {
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
						(name.clone(), f(name, alias, DependencyEntry::Inline(info), case.clone()))
					},
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
						(name.clone(), f(name, alias, DependencyEntry::Table(info), case.clone()))
					},
					None => continue,
					info => {
						warn!("Unsupported dependency format for {}. Format must be InlinedTable/Table, not {}", key, get_type_of(&info));
						(key.clone(), DependencyAction::Untouched)
					},
				};

				match action {
					DependencyAction::Remove => {
						t.remove(&name);
						removed.push(name);
					},
					DependencyAction::Untouched => { /* nop */ },
					_ => {
						counter += 1;
					},
				}
			}
		}
	}

	if !removed.is_empty() {
		if let Some(Item::Table(features)) = root.get_mut("features") {
			let keys = Vec::from_iter(features.iter().map(|(k, _v)| k.to_owned()));
			for feat in keys {
				if let Some(Item::Value(Value::Array(deps))) = features.get_mut(&feat) {
					let mut to_remove = Vec::new();
					for (idx, dep) in deps.iter().enumerate() {
						if let Value::String(s) = dep {
							if let Some(s) = s.value().trim().split('/').next() {
								if removed.contains(&s.to_owned()) {
									to_remove.push(idx);
								}
							}
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

pub(crate) fn handle_empty_package_is_failures<T>(
	packages: &Vec<T>,
	empty_package_is_failure: bool,
) -> anyhow::Result<()> {
	if packages.is_empty() {
		let empty_package_action = empty_package_bool_to_action(empty_package_is_failure);
		match empty_package_action {
			EmptyPackage::Ignore => {
				println!("No packages selected. All good. Exiting.");
				return Ok(());
			},
			EmptyPackage::Fail => {
				anyhow::bail!("No packages matching criteria. Exiting");
			},
		}
	}
	Ok(())
}

pub(crate) fn make_pkg_predicate(
	gctx: &GlobalContext,
	ws: &Workspace<'_>,
	args: PackageSelectOptions,
) -> Result<impl Fn(&Package) -> bool, anyhow::Error> {
	let PackageSelectOptions {
		packages,
		skip,
		ignore_pre_version,
		ignore_publish,
		changed_since,
		include_pre_deps,
	} = args;

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

	let publish = move |p: &Package| {
		// If publish is set to false or any registry, it is ignored by default
		// unless overriden.
		let value = ignore_publish || p.publish().is_none();

		trace!("{:}.publish={}", p.name(), value);
		value
	};
	let check_version = move |p: &Package| include_pre_deps && !p.version().pre.is_empty();

	let changed = if let Some(changed_since) = &changed_since {
		if !skip.is_empty() || !ignore_pre_version.is_empty() {
			anyhow::bail!(
                "-c/--changed-since is mutually exclusive to using -s/--skip and -i/--ignore-version-pre"
            );
		}
		Some(crate::util::changed_packages(&gctx, ws, changed_since)?)
	} else {
		None
	};

	Ok(move |p: &Package| {
		if !publish(p) {
			return false;
		}

		if let Some(changed) = &changed {
			return changed.contains(p) || check_version(p);
		}

		if !packages.is_empty() {
			trace!("going for matching against {:?}", packages);
			let name = p.name();
			if packages.iter().any(|r| r.is_match(&name)) {
				return true;
			}
			return check_version(p);
		}

		if !skip.is_empty() || !ignore_pre_version.is_empty() {
			let name = p.name();
			if skip.iter().any(|r| r.is_match(&name)) {
				return false;
			}
			if !p.version().pre.is_empty() &&
				ignore_pre_version.contains(&p.version().pre.as_str().to_owned())
			{
				return false;
			}
		}

		true
	})
}
