// Imperative shell: entry point.

mod config;

use clap::Parser;

fn main() -> eyre::Result<()> {
    let cli_args = config::CliArgs::parse();
    let config = config::load_config(cli_args)?;
    println!("{config:#?}");
    Ok(())
}
