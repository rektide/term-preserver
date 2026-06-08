// Imperative shell: entry point.

mod config;
mod osc;
mod prompt;
mod proxy;
mod signals;
mod terminal;

use std::io;
use std::os::fd::AsRawFd;

use clap::Parser;

fn install_panic_hook() {
    let stdin_fd = io::stdin().as_raw_fd();
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(stdin_fd) };
        if let Ok(current) = nix::sys::termios::tcgetattr(borrowed) {
            let mut cooked = current;
            nix::sys::termios::cfmakeraw(&mut cooked);
            if nix::sys::termios::tcgetattr(borrowed).is_ok() {
                let _ = nix::sys::termios::tcsetattr(
                    borrowed,
                    nix::sys::termios::SetArg::TCSANOW,
                    &cooked,
                );
            }
        }
        prev(info);
    }));
}

fn main() -> eyre::Result<()> {
    install_panic_hook();
    let cli_args = config::CliArgs::parse();
    let config = config::load_config(cli_args)?;
    let code = proxy::run(config)?;
    std::process::exit(code);
}
