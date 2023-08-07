use crate::{
	cli::{make_pkg_predicate, VersionCommand},
	util::{
		edit_each, edit_each_dep, members_deep, DependencyAction, DependencyEntry,
		DependencySection,
	},
};
use anyhow::Context;
use cargo::core::{package::Package, Workspace};
use log::trace;
use semver::{BuildMetadata, Prerelease, Version, VersionReq};
use std::collections::HashMap;
use toml_edit::{Entry, Item, Value};

fn check_for_update(
	name: String,
	wrap: DependencyEntry<'_>,
	updates: &HashMap<String, Version>,
	section: DependencySection,
	force_update: bool,
) -> DependencyAction {
	let new_version = if let Some(v) = updates.get(&name) {
		v
	} else {
		return DependencyAction::Untouched; // we do not care about this entry
	};

	match wrap {
		DependencyEntry::Inline(info) => {
			if !info.contains_key("path") {
				return DependencyAction::Untouched; // entry isn't local
			}

			trace!("We changed the version of {:} to {:}", name, new_version);
			// this has been changed.
			if let Some(v_req) = info.get_mut("version") {
				let r = v_req
					.as_str()
					.ok_or_else(|| anyhow::anyhow!("Version must be string"))
					.and_then(|s| VersionReq::parse(s).context("Parsing failed"))
					.expect("Cargo enforces us using semver versions. qed");
				if force_update || !r.matches(new_version) {
					trace!("Versions don't match anymore, updating.");
					*v_req = Value::from(format!("{:}", new_version)).decorated(" ", "");
					return DependencyAction::Mutated;
				}
			} else if section == DependencySection::Dev {
				trace!("No version found on dev dependency, ignoring.");
				return DependencyAction::Untouched;
			} else {
				// not yet present, we force set.
				trace!("No version found, setting.");
				// having a space here means we formatting it nicer inline
				info.get_or_insert(
					" version",
					Value::from(format!("{:}", new_version)).decorated(" ", " "),
				);
				return DependencyAction::Mutated;
			}
		},
		DependencyEntry::Table(info) => {
			if !info.contains_key("path") {
				return DependencyAction::Untouched; // entry isn't local
			}
			if let Some(new_version) = updates.get(&name) {
				trace!("We changed the version of {:} to {:}", name, new_version);
				// this has been changed.
				if let Some(v_req) = info.get("version") {
					let r = v_req
						.as_str()
						.ok_or_else(|| anyhow::anyhow!("Version must be string"))
						.and_then(|s| VersionReq::parse(s).context("Parsing failed"))
						.expect("Cargo enforces us using semver versions. qed");
					if !force_update && r.matches(new_version) {
						return DependencyAction::Untouched;
					}
					trace!("Versions don't match anymore, updating.");
				} else if section == DependencySection::Dev {
					trace!("No version found on dev dependency {:}, ignoring.", name);
					return DependencyAction::Untouched;
				} else {
					trace!("No version found, setting.");
				}
				info["version"] =
					Item::Value(Value::from(format!("{:}", new_version)).decorated(" ", ""));
				return DependencyAction::Mutated;
			}
		},
	}
	DependencyAction::Untouched
}

/// For packages matching predicate set to mapper given version, if any. Update all members
/// dependencies if necessary.
pub fn set_version<M, P>(
	ws: &Workspace<'_>,
	predicate: P,
	mapper: M,
	force_update: bool,
) -> Result<(), anyhow::Error>
where
	P: Fn(&Package) -> bool,
	M: Fn(&Package) -> Option<Version>,
{
	let c = ws.config();

	let updates = HashMap::<String, Version>::from_iter(
		edit_each(members_deep(ws).iter().filter(|p| predicate(p)), |p, doc| {
			Ok(mapper(p).map(|nv_version| {
				c.shell()
					.status(
						"Bumping",
						format!("{:}: {:} -> {:}", p.name(), p.version(), nv_version),
					)
					.expect("Writing to the shell would have failed before. qed");
				doc["package"]["version"] =
					Item::Value(Value::from(nv_version.to_string()).decorated(" ", ""));
				(p.name().as_str().to_owned(), nv_version)
			}))
		})?
		.into_iter()
		.flatten(),
	);

	c.shell().status("Updating", "Dependency tree")?;
	edit_each(members_deep(ws).iter(), |p, doc| {
		c.shell().status("Updating", p.name())?;
		let root = doc.as_table_mut();
		let mut updates_count = 0;
		updates_count += edit_each_dep(root, |name, _, wrap, section| {
			check_for_update(name, wrap, &updates, section, force_update)
		});

		if let Entry::Occupied(occupied) = root.entry("target") {
			if let Item::Table(table) = occupied.get() {
				let keys = Vec::from_iter(table.iter().filter_map(|(k, v)| {
					if v.is_table() {
						Some(k.to_owned())
					} else {
						None
					}
				}));

				for k in keys {
					if let Some(Item::Table(root)) = root.get_mut(&k) {
						updates_count += edit_each_dep(root, |a, _, b, c| {
							check_for_update(a, b, &updates, c, force_update)
						});
					}
				}
			}
		}
		let status = "Done";
		let mut status_message = format!("{} dependencies updated", updates_count);
		if updates_count == 0 {
			status_message = "No dependency updates".to_owned();
		} else if updates_count == 1 {
			status_message = "One dependency updated".to_owned();
		}
		c.shell().status(status, status_message)?;
		Ok(())
	})?;

	Ok(())
}

fn bump_major_version(v: &mut Version) {
	v.major += 1;
	v.minor = 0;
	v.patch = 0;
}

fn bump_minor_version(v: &mut Version) {
	v.minor += 1;
	v.patch = 0;
}

fn bump_patch_version(v: &mut Version) {
	// 0.0.x means each patch is breaking, see:
	// https://doc.rust-lang.org/cargo/reference/semver.html#change-categories
	v.patch += 1;
}

/// Adjust the version of the crate according to the given version adjustment command
pub fn adjust_version(ws: &Workspace<'_>, cmd: VersionCommand) -> Result<(), anyhow::Error> {
	match cmd {
		VersionCommand::Set { pkg_opts, force_update, version } => {
			let predicate = make_pkg_predicate(ws, pkg_opts)?;
			set_version(ws, |p| predicate(p), |_| Some(version.clone()), force_update)
		},
		VersionCommand::BumpPre { pkg_opts, force_update } => {
			let predicate = make_pkg_predicate(ws, pkg_opts)?;
			set_version(
				ws,
				|p| predicate(p),
				|p| {
					let mut v = p.version().clone();
					if v.pre.is_empty() {
						v.pre = Prerelease::new("1").expect("Static will work");
					} else if let Ok(num) = v.pre.as_str().parse::<u32>() {
						v.pre = Prerelease::new(&format!("{}", num + 1)).expect("Known to work");
					} else {
						let mut items =
							Vec::from_iter(v.pre.as_str().split('.').map(|s| s.to_string()));
						if let Some(num) = items.last().and_then(|u| u.parse::<u32>().ok()) {
							let _ = items.pop();
							items.push(format!("{}", num + 1));
						} else {
							items.push("1".to_owned());
						}
						if let Ok(pre) = Prerelease::new(&items.join(".")) {
							v.pre = pre;
						} else {
							return None;
						}
					}
					Some(v)
				},
				force_update,
			)
		},
		VersionCommand::BumpPatch { pkg_opts, force_update } => {
			let predicate = make_pkg_predicate(ws, pkg_opts)?;
			set_version(
				ws,
				|p| predicate(p),
				|p| {
					let mut v = p.version().clone();
					v.pre = Prerelease::EMPTY;
					bump_patch_version(&mut v);
					Some(v)
				},
				force_update,
			)
		},
		VersionCommand::BumpMinor { pkg_opts, force_update } => {
			let predicate = make_pkg_predicate(ws, pkg_opts)?;
			set_version(
				ws,
				|p| predicate(p),
				|p| {
					let mut v = p.version().clone();
					v.pre = Prerelease::EMPTY;
					bump_minor_version(&mut v);
					Some(v)
				},
				force_update,
			)
		},
		VersionCommand::BumpMajor { pkg_opts, force_update } => {
			let predicate = make_pkg_predicate(ws, pkg_opts)?;
			set_version(
				ws,
				|p| predicate(p),
				|p| {
					let mut v = p.version().clone();
					v.pre = Prerelease::EMPTY;
					bump_major_version(&mut v);
					Some(v)
				},
				force_update,
			)
		},
		VersionCommand::BumpBreaking { pkg_opts, force_update } => {
			let predicate = make_pkg_predicate(ws, pkg_opts)?;
			set_version(
				ws,
				|p| predicate(p),
				|p| {
					let mut v = p.version().clone();
					v.pre = Prerelease::EMPTY;
					if v.major != 0 {
						bump_major_version(&mut v);
					} else if v.minor != 0 {
						bump_minor_version(&mut v);
					} else {
						bump_patch_version(&mut v);
						// no helper, have to reset the metadata ourselves
						v.build = BuildMetadata::EMPTY;
					}
					Some(v)
				},
				force_update,
			)
		},
		VersionCommand::BumpToDev { pkg_opts, force_update, pre_tag } => {
			let predicate = make_pkg_predicate(ws, pkg_opts)?;
			let pre_val = pre_tag.unwrap_or_else(|| "dev".to_owned());
			set_version(
				ws,
				|p| predicate(p),
				|p| {
					let mut v = p.version().clone();
					if v.major != 0 {
						bump_major_version(&mut v);
					} else if v.minor != 0 {
						bump_minor_version(&mut v);
					} else {
						bump_patch_version(&mut v);
						// no helper, have to reset the metadata ourselves
						v.build = BuildMetadata::EMPTY;
					}
					// force the pre
					v.pre = Prerelease::new(&pre_val.clone()).expect("Static or expected to work");
					Some(v)
				},
				force_update,
			)
		},
		VersionCommand::SetPre { pre, pkg_opts, force_update } => {
			let predicate = make_pkg_predicate(ws, pkg_opts)?;
			set_version(
				ws,
				|p| predicate(p),
				|p| {
					let mut v = p.version().clone();
					v.pre = Prerelease::new(&pre.clone()).expect("Static or expected to work");
					Some(v)
				},
				force_update,
			)
		},
		VersionCommand::SetBuild { meta, pkg_opts, force_update } => {
			let predicate = make_pkg_predicate(ws, pkg_opts)?;
			set_version(
				ws,
				|p| predicate(p),
				|p| {
					let mut v = p.version().clone();
					v.build = BuildMetadata::new(&meta.clone())
						.expect("The meta you provided couldn't be parsed");
					Some(v)
				},
				force_update,
			)
		},
		VersionCommand::Release { pkg_opts, force_update } => {
			let predicate = make_pkg_predicate(ws, pkg_opts)?;
			set_version(
				ws,
				|p| predicate(p),
				|p| {
					let mut v = p.version().clone();
					v.pre = Prerelease::EMPTY;
					v.build = BuildMetadata::EMPTY;
					Some(v)
				},
				force_update,
			)
		},
	}
}
