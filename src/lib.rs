//! `term-preserver` — a small, synchronous PTY proxy that observes terminal
//! title and color sequences.
//!
//! The proxy sits between your terminal and a child process, relaying every
//! byte unchanged while a streaming parser watches the PTY→stdout stream for
//! OSC sequences. Captured state is tracked in distinct slots and surfaced on
//! child exit for restore and display.
//!
//! ## Core pieces
//!
//! - [`osc`] — the streaming OSC parser. Pure state machine; produces typed
//!   [`osc::OscCapture`]s and classifies each byte (emit/hold/flush) so the
//!   relay can strip filtered titles byte-for-byte.
//! - [`color`] — parses terminal color specs (`rgb:`, `rgbi:`, `#`) into a
//!   normalized 24-bit value alongside the raw text.
//! - [`session`] — the imperative shell: PTY child session, relay threads, and
//!   the thread-shared [`session::ObservedState`] that holds the captured slots
//!   (window title, icon name, palette/special colors, fg/bg/cursor).
//! - [`filter`] — regex title filters with independent `strip`/`ignore`
//!   actions, gated behind the default-on `regex` Cargo feature.
//! - [`prompt`] — pure formatting: the retry prompt, OSC re-emit
//!   reconstruction, and the visible "gone" line.
//! - [`tmux`] — the tmux slot (`TmuxSample`/`TmuxState`) plus, behind the
//!   `tmux` feature, a sampler that polls the local tmux server by client PID.
//! - [`proxy`] — the [`proxy::RetryLoop`] orchestrator tying it together.

pub mod color;
pub mod completion;
pub mod config;
pub mod filter;
pub mod launch;
pub mod launcher;
pub mod osc;
pub mod prompt;
pub mod proxy;
pub mod runtime;
pub mod session;
pub mod signals;
pub mod terminal;
pub mod tmux;

/// Shared entry for the `term-preserver` and `tper` binaries: parse, handle
/// the hidden worker/completion lanes, then run the proxy.
pub fn bin_entry() -> eyre::Result<()> {
    use clap::Parser;
    let cli_args = config::CliArgs::parse();
    if let Some(request) = &cli_args.launch_worker {
        return launcher::execute_worker(request);
    }
    // The hidden completion lane observes state only; it shares the ordinary
    // config merge so inventories honor the same root/socket/program layers.
    let completion = cli_args.completion_candidates;
    let config = config::load_config(cli_args)?;
    if let Some(kind) = completion {
        return kind.run(&config.launch);
    }
    let code = proxy::run(config)?;
    std::process::exit(code);
}
