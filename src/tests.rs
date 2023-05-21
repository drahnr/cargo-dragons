use super::*;

use super::cli::Args;
use clap::Parser;

use assert_matches::assert_matches;

#[test]
fn argparse_1() {
	let args = Args::try_parse_from(
		"cargo-dragons set version 1.0.0 -p crateA -p crateB".split_ascii_whitespace(),
	);
	assert_matches!(args.unwrap().cmd, cli::Command::Set { pkg_opts, root_key, name, value } => {
		assert_eq!(Vec::from_iter(pkg_opts.packages.into_iter().map(|x| x.to_string())), vec!["crateA", "crateB"]);
		assert_eq!(name, "version");
		assert_eq!(value, "1.0.0");
	});
}

#[test]
fn argparse_2() {
	let args = Args::try_parse_from(
		"cargo-dragons set -p crateA -p crateB version 1.0.0".split_ascii_whitespace(),
	);
	assert_matches!(args.unwrap().cmd, cli::Command::Set { pkg_opts, root_key, name, value } => {
		assert_eq!(Vec::from_iter(pkg_opts.packages.into_iter().map(|x| x.to_string())), vec!["crateA", "crateB"]);
		assert_eq!(name, "version");
		assert_eq!(value, "1.0.0");
	});
}

#[test]
fn argparse_3() {
	let args = Args::try_parse_from(vec![
		"cargo-dragons",
		"set",
		"-p",
		"crate0",
		"authors",
		"Bernhard Schuster <bernhard@ahoi.io>",
	]);
	assert_matches!(args.unwrap().cmd, cli::Command::Set { pkg_opts, root_key, name, value } => {
		assert_eq!(Vec::from_iter(pkg_opts.packages.into_iter().map(|x| x.to_string())), vec!["crate0"]);
		assert_eq!(name, "authors");
		assert_eq!(value, "Bernhard Schuster <bernhard@ahoi.io>");
	});
}
