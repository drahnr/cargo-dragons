use crate::util::{edit_each, edit_each_dep, members_deep, DependencyAction, DependencyEntry};
use cargo::{
	core::{package::Package, Workspace},
	GlobalContext,
};
use log::trace;
use std::collections::HashMap;
use toml_edit::{Item, Value};

fn check_for_update(
	name: String,
	wrap: DependencyEntry<'_>,
	updates: &HashMap<String, String>,
) -> DependencyAction {
	let new_name = if let Some(v) = updates.get(&name) {
		v
	} else {
		return DependencyAction::Untouched; // we do not care about this entry
	};

	match wrap {
		DependencyEntry::Inline(info) => {
			if !info.contains_key("path") {
				return DependencyAction::Untouched; // entry isn't local
			}

			trace!("We renamed {:} to {:}", name, new_name);
			info.get_or_insert(" package", Value::from(new_name.to_string()).decorated(" ", " "));

			DependencyAction::Mutated
		},
		DependencyEntry::Table(info) => {
			if !info.contains_key("path") {
				return DependencyAction::Untouched; // entry isn't local
			}

			info["package"] = Item::Value(Value::from(new_name.to_string()).decorated(" ", ""));

			DependencyAction::Mutated
		},
	}
}

/// For packages matching predicate set to mapper given version, if any. Update all members
/// dependencies if necessary.
pub fn rename<M, P>(
	gctx: &GlobalContext,
	ws: &Workspace<'_>,
	predicate: P,
	mapper: M,
) -> Result<(), anyhow::Error>
where
	P: Fn(&Package) -> bool,
	M: Fn(&Package) -> Option<String>,
{
	let updates = HashMap::<String, String>::from_iter(
		edit_each(members_deep(gctx, ws).iter().filter(|p| predicate(p)), |p, doc| {
			Ok(mapper(p).map(|new_name| {
				gctx.shell()
					.status("Renaming", format!("{:} -> {:}", dbg!(&p).name(), new_name))
					.expect("Writing to the shell would have failed before. qed");
				doc["package"]["name"] =
					Item::Value(Value::from(new_name.to_string()).decorated(" ", ""));
				(p.name().as_str().to_owned(), new_name)
			}))
		})?
		.into_iter()
		.flatten(),
	);

	if updates.is_empty() {
		gctx.shell().status("Done", "No changed applied")?;
		return Ok(());
	}

	gctx.shell().status("Updating", "Dependency tree")?;
	edit_each(members_deep(gctx, ws).iter(), |p, doc| {
		gctx.shell().status("Updating", p.name())?;
		let root = doc.as_table_mut();
		let mut updates_count = 0;
		updates_count += edit_each_dep(root, |a, _, b, _| check_for_update(a, b, &updates));

		if let Some(Item::Table(table)) = root.get_mut("target") {
			let keys = Vec::from_iter(table.iter().filter_map(|(k, v)| {
				if v.is_table() {
					Some(k.to_owned())
				} else {
					None
				}
			}));

			for k in keys {
				if let Some(Item::Table(root)) = table.get_mut(&k) {
					updates_count +=
						edit_each_dep(root, |a, _, b, _| check_for_update(a, b, &updates));
				}
			}
		}

		if updates_count == 0 {
			gctx.shell().status("Done", "No dependency updates")?;
		} else if updates_count == 1 {
			gctx.shell().status("Done", "One dependency updated")?;
		} else {
			gctx.shell().status("Done", format!("{} dependencies updated", updates_count))?;
		}

		Ok(())
	})?;

	Ok(())
}
