// Imperative shell: entry point.

mod config;
mod osc;
mod proxy;
mod terminal;

use clap::Parser;

fn main() -> eyre::Result<()> {
    let cli_args = config::CliArgs::parse();
    let config = config::load_config(cli_args)?;
    let (code, _title_state) = proxy::run(config)?;
    std::process::exit(code);
}
