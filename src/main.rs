use anyhow::Result;
use clap::Parser;
use codex_migrate::cli;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    cli::run(cli::Cli::parse())
}
