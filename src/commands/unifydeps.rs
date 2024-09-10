use crate::util::{edit_each, members_deep};

use anyhow::{bail, Context};
use cargo::core::package::Package;

use cargo::{core::Workspace, GlobalContext};
use cargo_util_schemas::manifest::TomlManifest;
use toml_edit::{Formatted, InlineTable, Item, Key, Table, TableLike, Value};

trait SortableTableKeysBy {
	fn sort_values_by<F>(&mut self, compare: F)
	where
		F: FnMut(&Key, &Key) -> std::cmp::Ordering;
}

impl SortableTableKeysBy for Table {
	fn sort_values_by<F>(&mut self, mut compare: F)
	where
		F: FnMut(&Key, &Key) -> std::cmp::Ordering,
	{
		Table::sort_values_by(self, move |k1, _, k2, _| compare(k1, k2));
	}
}

impl SortableTableKeysBy for InlineTable {
	fn sort_values_by<F>(&mut self, mut compare: F)
	where
		F: FnMut(&Key, &Key) -> std::cmp::Ordering,
	{
		InlineTable::sort_values_by(self, move |k1, _, k2, _| compare(k1, k2));
	}
}

fn log(gctx: &GlobalContext, packet: &str, dep: &str, ver: &str) {
	gctx.shell()
		.status("Unifying dependency", format!("Unified {dep} @ {ver} of {packet} -> workspace"))
		.expect("Writing to the shell would have failed before. qed");
}

fn replace_version_by_workspace<T: TableLike + SortableTableKeysBy + std::fmt::Debug>(
	gctx: &GlobalContext,
	packet: &str,
	dep_name: &str,
	tablelike: &mut T,
) {
	let Some(version) = tablelike.remove("version") else { return };
	let suffix = match tablelike.len() {
		0 | 1 => " ",
		_ => "",
	};
	tablelike.insert(
		"workspace",
		Item::Value(Value::Boolean(Formatted::new(true)).decorated(" ", suffix)),
	);
	// ensure `workspace = true` is the first key
	tablelike.sort_values_by(|key1, _key2| {
		if key1.get() == "workspace" {
			std::cmp::Ordering::Less
		} else {
			std::cmp::Ordering::Equal
		}
	});

	log(gctx, packet, dep_name, version.as_str().unwrap_or_default());
}

/// Deactivate the Dev Dependencies Section of the given toml
pub fn unify_dependencies<P>(
	gctx: &GlobalContext,
	ws: &mut Workspace<'_>,
	predicate: P,
) -> Result<(), anyhow::Error>
where
	P: Fn(&Package) -> bool,
{
	let manifest_path = ws.root_manifest().to_path_buf();
	let root_ws_content = std::fs::read_to_string(&manifest_path)
		.context(format!("Failed to read root manifest {}", manifest_path.display()))?;
	let dependencies_to_unify: TomlManifest = toml::from_str(dbg!(&root_ws_content))?;
	let Some(dependencies_to_unify) = dependencies_to_unify.workspace.unwrap().dependencies else {
		bail!("No workspace level dependencies, nothing to unify")
	};

	edit_each(members_deep(gctx, ws).iter().filter(|p| predicate(p)), |p, doc| {
		let per_table = |deps: &mut Item| {
			let Some(deps) = deps.as_table_mut() else { return Ok(()) };

			for (dep_name, _dep) in &dependencies_to_unify {
				match deps.entry(dep_name.as_str()) {
					toml_edit::Entry::Vacant(_) => {},
					toml_edit::Entry::Occupied(mut occ) => {
						let occ = occ.get_mut();

						if let Some(tab) = occ.as_table_mut() {
							// [dependencies.foo]
							replace_version_by_workspace(gctx, p.name().as_str(), dep_name, tab);
						} else if let Some(value) = occ.as_value_mut() {
							// foo = ..
							match value {
								Value::InlineTable(tab) => {
									// foo = { version = "0.1" , feature ... }
									replace_version_by_workspace(
										gctx,
										p.name().as_str(),
										dep_name,
										tab,
									)
								},
								occ @ Value::String(_) => {
									// foo = "0.1"
									let mut tab = InlineTable::new();
									tab.insert(
										"workspace",
										Value::Boolean(Formatted::new(true)).decorated(" ", " "),
									);
									let version = occ.as_str().unwrap_or_default().to_string();
									log(gctx, p.name().as_str(), dep_name, &version);
									*occ = Value::InlineTable(tab);
								},
								unknown => anyhow::bail!("Unknown {}", unknown),
							}
						} else {
							gctx.shell()
								.warn("Neither a table nor value, unable to deal with.")
								.unwrap();
							continue;
						}
					},
				}
			}
			Ok(())
		};
		per_table(&mut doc["dependencies"])?;
		per_table(&mut doc["dev-dependencies"])?;
		Ok(())
	})?;

	// if updates.is_empty() {
	// gctx.shell().status("Done", "No changed applied")?;
	// }

	Ok(())
}
