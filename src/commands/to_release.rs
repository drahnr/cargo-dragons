use crate::util::members_deep;
use cargo::{
	GlobalContext,
	core::{SourceId, Workspace, package::Package},
	sources::{registry::RegistrySource, source::Source},
	util::interning::InternedString,
};
#[cfg(not(test))]
use cargo::{core::Dependency, sources::source::QueryKind};
use log::{trace, warn};
use petgraph::{
	Directed, Graph,
	dot::{self, Dot},
	graph::{EdgeReference, NodeIndex},
	visit::EdgeRef,
};
use std::{
	collections::{HashMap, HashSet},
	fs::OpenOptions,
	io::Write,
	path::PathBuf,
};

/// Generate the packages we should be releasing
pub fn packages_to_release<F, D>(
	gctx: &GlobalContext,
	ws: &Workspace<'_>,
	predicate: F,
	write_dot_graph: D,
) -> Result<Vec<Package>, anyhow::Error>
where
	F: Fn(&Package) -> bool,
	D: Into<Option<PathBuf>>,
{
	packages_to_release_inner::<F, D>(gctx, ws, predicate, write_dot_graph).map_err(
		|ErrorWithCycles(cycles, e)| {
			let named = Vec::from_iter(
				cycles.iter().map(|cycle| cycle.iter().map(|pkg| pkg.name().as_str())),
			);
			e.context(format!("Cycles: {:?}", named))
		},
	)
}

type DependencyCycle = Vec<Package>;

/// Error with additional cycle annotations.
#[derive(Debug)]
struct ErrorWithCycles(Vec<DependencyCycle>, anyhow::Error);

impl<T: Into<anyhow::Error>> From<T> for ErrorWithCycles {
	fn from(src: T) -> Self {
		ErrorWithCycles(Vec::new(), src.into())
	}
}

fn packages_to_release_inner<F, D>(
	gctx: &GlobalContext,
	ws: &Workspace<'_>,
	predicate: F,
	write_dot_graph: D,
) -> Result<Vec<Package>, ErrorWithCycles>
where
	F: Fn(&Package) -> bool,
	D: Into<Option<PathBuf>>,
{
	let lock =
		gctx.acquire_package_cache_lock(cargo::util::cache_lock::CacheLockMode::MutateExclusive)?;

	// inspired by the work of `cargo-publish-all`: https://gitlab.com/torkleyy/cargo-publish-all
	gctx.shell()
		.status("Resolving", "Dependency Tree")
		.expect("Writing to Shell doesn't fail");

	let mut graph = Graph::<Package, (), Directed, u32>::new();
	let members = members_deep(gctx, ws);

	let (members, to_ignore): (Vec<_>, Vec<_>) = members.iter().partition(|m| predicate(m));

	let ignored = HashSet::<InternedString>::from_iter(to_ignore.into_iter().map(|m| m.name()));

	gctx.shell()
		.status("Syncing", "Versions from crates.io")
		.expect("Writing to Shell doesn't fail");

	let mut already_published = HashSet::new();
	let registry = RegistrySource::remote(
		SourceId::crates_io(gctx).expect(
			"Your main registry (usually crates.io) can't be read. Please check your .cargo/config",
		),
		&Default::default(),
		gctx,
	)
	.expect("Failed getting remote registry");

	registry.invalidate_cache();

	for m in members.iter() {
		if is_published_to_registry(&registry, m) {
			already_published.insert(m.name());
		}
	}

	// drop the global package lock
	drop(lock);

	let map =
		HashMap::<InternedString, NodeIndex>::from_iter(members.iter().filter_map(|&member| {
			if ignored.contains(&member.name()) || already_published.contains(&member.name()) {
				return None;
			}
			Some((member.name(), graph.add_node(member.clone())))
		}));

	let default_registry = SourceId::crates_io(gctx)?;
	for member in members {
		let current_index = match map.get(&member.name()) {
			Some(i) => i,
			_ => continue, // ignore entries we are not expected to publish
		};

		for dep in member.dependencies() {
			if let Some(dep_index) = map.get(&dep.package_name()) {
				graph.add_edge(*current_index, *dep_index, ());
			} else if already_published.contains(&dep.package_name()) {
				trace!("All good, it's on crates.io");
			} else {
				// we are looking at a dependency, we won't include in the set of
				// ones we are about to publish. Let's make sure, this won't block
				// us from doing so though.
				trace!("Checking dependency for problems: {}", dep.package_name());
				let source = dep.source_id();
				if source == default_registry {
					trace!("All good, it's on crates.io")
				} else if source.is_path() && dep.is_locked() {
					// this is a pretty big indicator that something is going to fail later...
					if ignored.contains(&dep.package_name()) {
						warn!(
							"{} lock depends on {}, which is expected to not be published. This might fail.",
							member.name(),
							dep.package_name()
						)
					}
				}
			}
		}
	}

	// cannot use `toposort` for graphs that are cyclic in a undirected sense
	// but are not in a directed way
	let mut cycles = Vec::new();
	let mut toposorted_indices = Vec::new();
	let strongly_connected_sets = petgraph::algo::kosaraju_scc(&graph);
	for strongly_connected in strongly_connected_sets {
		match strongly_connected.len() {
			0 => unreachable!("Strongly connected components are at least size 1. qed"),
			1 => toposorted_indices.push(strongly_connected[0]),
			_ => cycles.push(strongly_connected),
		}
	}

	if let Some(dest) = write_dot_graph.into() {
		let mut dest = OpenOptions::new().create(true).truncate(true).write(true).open(dest)?;
		graphviz(&graph, &cycles, &mut dest)?;
	}

	if !cycles.is_empty() {
		assert!(petgraph::algo::is_cyclic_directed(&graph));
		let cycles = cycles
			.iter()
			.map(|nodes| {
				nodes
					.iter()
					.map(|i| graph.node_weight(*i).unwrap())
					.cloned()
					.collect::<Vec<_>>()
			})
			.collect::<Vec<_>>();
		return Err(ErrorWithCycles(cycles, anyhow::anyhow!("Contains cycles")));
	}

	// the output of `kosaraju_scc` is in reverse topological order, leafs first, which matches

	let packages = Vec::from_iter(
		toposorted_indices.into_iter().map(|i| graph.node_weight(i).unwrap().clone()),
	);

	Ok(packages)
}

#[cfg(not(test))]
fn is_published_to_registry(registry: &RegistrySource<'_>, package: &Package) -> bool {
	if std::env::var_os("CARGO_NET_OFFLINE").is_some() {
		return false;
	}

	let dep = Dependency::parse(
		package.name(),
		Some(&package.version().to_string()),
		registry.source_id(),
	)
	.expect("Parsing our dependency doesn't fail");

	let mut found = false;
	futures::executor::block_on(registry.query(&dep, QueryKind::Exact, &mut |_| {
		found = true;
	}))
	.expect("Querying the local registry doesn't fail");
	found
}

#[cfg(test)]
fn is_published_to_registry(_registry: &RegistrySource<'_>, _package: &Package) -> bool {
	false
}

/// Render a graphviz (aka dot graph) to a file.
fn graphviz<'i, I: IntoIterator<Item = &'i Vec<NodeIndex>>, W: Write>(
	graph: &Graph<Package, (), Directed, u32>,
	cycles: I,
	dest: &mut W,
) -> anyhow::Result<()> {
	let cycle_indices =
		HashSet::<NodeIndex>::from_iter(cycles.into_iter().flat_map(|y| y.iter()).copied());
	let config = &[dot::Config::EdgeNoLabel, dot::Config::NodeNoLabel][..];
	let get_edge_attributes =
		|_graph: &Graph<Package, (), Directed, u32>, edge_ref: EdgeReference<'_, ()>| -> String {
			let source = edge_ref.source();
			let target = edge_ref.target();
			if cycle_indices.contains(&target) && cycle_indices.contains(&source) {
				r#"color=red"#
			} else {
				""
			}
			.to_owned()
		};
	let get_node_attributes =
		|_graph: &Graph<Package, (), Directed, u32>, (idx, pkg): (NodeIndex, &Package)| -> String {
			let color = if cycle_indices.contains(&idx) { "color=red" } else { "" };
			format!(r#"label="{}:{}" {}"#, pkg.name(), pkg.version(), color)
		};

	let dot = Dot::with_attr_getters(graph, config, &get_edge_attributes, &get_node_attributes);
	dest.write_all(format!("{:?}", &dot).as_bytes())?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use anyhow::Result;
	use cargo::GlobalContext;
	use itertools::Itertools;
	use semver::Version;
	use std::{fs, path::Path};

	#[derive(Default, Debug, Clone)]
	struct Krate {
		name: &'static str,
		version: Option<Version>,
		dependencies: Vec<(&'static str, &'static str)>,
	}

	impl Krate {
		pub fn version(&mut self, major: u64, minor: u64, patch: u64) -> &mut Self {
			self.version = Some(Version::new(major, minor, patch));
			self
		}

		pub fn add_dependency(
			&mut self,
			dependency: &'static str,
			version_req: &'static str,
		) -> Result<&mut Self> {
			self.dependencies.push((dependency, version_req));
			Ok(self)
		}
	}

	#[derive(Default, Debug, Clone)]
	struct WorkspaceBuilder {
		krates: Vec<Krate>,
	}

	impl WorkspaceBuilder {
		pub fn add_crate(&mut self, name: &'static str) -> &mut Krate {
			let krate = Krate { name, version: None, dependencies: Vec::new() };
			self.krates.push(krate);
			self.krates.last_mut().unwrap()
		}

		pub fn build(
			self,
			base: impl AsRef<Path>,
		) -> Result<(&'static GlobalContext, Workspace<'static>)> {
			let base = base.as_ref();
			fs::create_dir_all(base)?;

			let members = self.krates.iter().map(|krate| format!(r#""{}""#, krate.name)).join(", ");
			fs::write(
				base.join("Cargo.toml"),
				format!(
					r#"
[workspace]
resolver = "3"
members = [{members}]
"#
				),
			)?;

			for krate in self.krates.iter() {
				let crate_dir = base.join(krate.name);
				fs::create_dir_all(crate_dir.join("src"))?;
				let dependencies = krate
					.dependencies
					.iter()
					.map(|(name, version)| {
						format!("{name} = {{ version = \"{version}\", path = \"../{name}\" }}")
					})
					.join("\n");
				fs::write(
					crate_dir.join("Cargo.toml"),
					format!(
						r#"
[package]
name = "{name}"
version = "{version}"
edition = "2024"
description = "{name}"
publish = false

[dependencies]
{dependencies}
"#,
						name = krate.name,
						version = krate.version.clone().expect("Must have version. qed"),
					),
				)?;
				fs::write(
					crate_dir.join("src/lib.rs"),
					format!("pub fn {}() {{}}\n", krate.name.replace('-', "_")),
				)?;
			}

			let gctx = Box::leak(Box::new(GlobalContext::default()?));
			let ws = Workspace::new(&base.join("Cargo.toml"), gctx)?;
			Ok((gctx, ws))
		}
	}

	/// Setup a diamond dependency graph and verify release order.
	#[test]
	fn diamond() -> Result<()> {
		let tmp = tempfile::tempdir()?;

		let mut wsb = WorkspaceBuilder::default();
		wsb.add_crate("top")
			.version(0, 1, 2)
			.add_dependency("dx", "1.11")?
			.add_dependency("dy", "15")?;
		wsb.add_crate("dx").version(1, 11, 111).add_dependency("closing", "1.6.4")?;
		wsb.add_crate("dy").version(15, 100, 0).add_dependency("closing", "1.6.1")?;
		wsb.add_crate("closing").version(1, 6, 9);

		let (gctx, ws) = wsb.build(tmp.path())?;
		let to_release =
			packages_to_release(gctx, &ws, |_pkg| true, tmp.path().join("diamond.dot"))
				.expect("There are no cycles in a diamond shaped, directed, dependency graph. qed");
		// must be in release order, so the leaf has to have a lower index, dependencies on the same
		// level are ordered by their reverse appearance in the members declaration
		assert_eq!(
			vec!["closing", "dy", "dx", "top"],
			to_release.iter().map(|pkg| pkg.name().as_str()).collect::<Vec<_>>()
		);
		Ok(())
	}

	#[test]
	fn circular() -> Result<()> {
		let tmp = tempfile::tempdir()?;

		let mut wsb = WorkspaceBuilder::default();
		wsb.add_crate("a").version(3, 0, 0).add_dependency("b", "*")?;
		wsb.add_crate("b").version(2, 0, 0).add_dependency("c", "*")?;
		wsb.add_crate("c").version(1, 0, 0).add_dependency("a", "*")?;

		let (gctx, ws) = wsb.build(tmp.path())?;
		let ErrorWithCycles(cycles, _err) =
			packages_to_release_inner(gctx, &ws, |_pkg| true, tmp.path().join("circular.dot"))
				.unwrap_err();
		assert_eq!(cycles.len(), 1);
		assert_eq!(cycles[0].len(), 3);
		// The start node is defined by the sequence in the members declaration
		assert_eq!(
			vec!["a", "b", "c"],
			cycles[0].iter().map(|pkg| pkg.name().as_str()).collect::<Vec<_>>()
		);
		Ok(())
	}
}
