// pattern: Imperative Shell

use clap::Parser;
use title_proxy::{config, proxy};

fn main() -> eyre::Result<()> {
    let cli_args = config::CliArgs::parse();
    let config = config::load_config(cli_args)?;
    let code = proxy::run(config)?;
    std::process::exit(code);
}
