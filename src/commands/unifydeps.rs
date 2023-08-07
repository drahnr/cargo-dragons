use crate::util::{edit_each, members_deep};

use cargo::core::package::Package;

use cargo::core::Workspace;
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

fn replace_version_by_workspace<T: TableLike + SortableTableKeysBy + std::fmt::Debug>(
	c: &cargo::Config,
	packet: &str,
	tablelike: &mut T,
) {
	let message = if let Some(v) = tablelike.remove("version") {
		format!("{:} : {:} -> workspace", packet, v)
	} else {
		format!("{:} : ? -> workspace", packet)
	};
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

	c.shell()
		.status("Unifying dependency", message)
		.expect("Writing to the shell would have failed before. qed");
}

/// Deactivate the Dev Dependencies Section of the given toml
pub fn unify_dependencies<P>(ws: &mut Workspace<'_>, predicate: P) -> Result<(), anyhow::Error>
where
	P: Fn(&Package) -> bool,
{
	let c = ws.config();
	let _manifest = ws.root_manifest();
	let Some(ws_config) = ws.load_workspace_config()? else {
		anyhow::bail!("Must be workspace");
	};
	let inh = ws_config.inheritable();
	let dependencies_to_unify = inh.dependencies()?;

	edit_each(members_deep(ws).iter().filter(|p| predicate(p)), |p, doc| {
		let per_table = |deps: &mut Item| {
			if let Some(deps) = deps.as_table_mut() {
				for dep in dependencies_to_unify.keys() {
					match deps.entry(dep) {
						toml_edit::Entry::Vacant(_) => {},
						toml_edit::Entry::Occupied(mut occ) => {
							let occ = occ.get_mut();

							if let Some(tab) = occ.as_table_mut() {
								// [dependencies.foo]
								replace_version_by_workspace(c, p.name().as_str(), tab);
							} else if let Some(value) = occ.as_value_mut() {
								// foo = ..
								match value {
									Value::InlineTable(tab) => {
										// foo = { version = "0.1" , feature ... }
										replace_version_by_workspace(c, p.name().as_str(), tab)
									},
									occ @ Value::String(_) => {
										// foo = "0.1"
										let mut tab = InlineTable::new();
										tab.insert(
											"workspace",
											Value::Boolean(Formatted::new(true))
												.decorated(" ", " "),
										);
										*occ = Value::InlineTable(tab);
									},
									unknown => anyhow::bail!("Unknown {}", unknown),
								}
							} else {
								c.shell()
									.warn("Neither a table nor value, unable to deal with.")
									.unwrap();
								continue;
							}
						},
					}
				}
			}
			Ok(())
		};
		per_table(&mut doc["dependencies"])?;
		per_table(&mut doc["dev-dependencies"])?;
		Ok(())
	})?;

	// if updates.is_empty() {
	// c.shell().status("Done", "No changed applied")?;
	// }

	Ok(())
}
