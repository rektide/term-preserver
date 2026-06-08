// Imperative shell: entry point.

mod config;
mod proxy;
mod terminal;

use clap::Parser;

fn main() -> eyre::Result<()> {
    let cli_args = config::CliArgs::parse();
    let config = config::load_config(cli_args)?;
    let code = proxy::run(config)?;
    std::process::exit(code);
}
