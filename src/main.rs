//! This crate provide release automation tooling for large monorepos

#![deny(unused_imports, missing_docs)]

use clap::Parser;
mod cli;
mod commands;
mod util;

#[cfg(test)]
mod tests;

use cli::Args;

fn main() -> Result<(), anyhow::Error> {
	let mut argv = Vec::new();
	let mut args = std::env::args();
	argv.extend(args.next());
	if let Some(h) = args.next() {
		if h != "dragons" {
			argv.push(h)
		}
	}
	argv.extend(args);
	let args = Args::parse_from(argv);
	cli::run(args)
}
