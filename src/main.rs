// pattern: Imperative Shell

use clap::Parser;
use term_preserver::{config, proxy};

fn main() -> eyre::Result<()> {
    let cli_args = config::CliArgs::parse();
    if let Some(request) = &cli_args.launch_worker {
        return term_preserver::launcher::execute_worker(request);
    }
    let config = config::load_config(cli_args)?;
    let code = proxy::run(config)?;
    std::process::exit(code);
}
