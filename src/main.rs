use clap::Parser;
mod cli;
mod commands;
mod util;

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
