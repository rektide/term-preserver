//! Read-only candidate inventories for shell completion, mirroring the tta
//! completion protocol. Queries observe state only: they never create, attach,
//! or switch a tmux session, never start a server (an absent server is an
//! empty session inventory), and never run a scaffold generator.
//!
//! Lookup configuration is the same merged [`LaunchConfig`] a launch uses, so
//! an inventory honors `--session-root`, `--tmux-socket`, `--config`, program
//! overrides, and the ambient environment exactly as the real invocation
//! would. Inventories are **local-only**: a remote transport (`--ssh`/`--mosh`)
//! answers with no host inventory rather than initiating a transport or
//! silently querying the wrong local server. The compiled backend list stays
//! available in every context because it never contacts a host.

use std::path::Path;
use std::process::Command;

use clap::ValueEnum;

use crate::launch::LaunchConfig;
use crate::launcher::{derive_socket, executable_present, project_root, SessionBackend};

/// Hidden `--completion-candidates` selection: one query per invocation,
/// printed one name per line on stdout. Errors go to stderr and leave stdout
/// empty, so a completion widget can silence them without hiding candidates.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum CompletionCandidates {
    /// Compiled `--session-backend` values.
    Backends,
    /// Immediate project directory basenames for session targets.
    Projects,
    /// Live tmux session names for session targets.
    Sessions,
    /// Configured tta scaffold generator names for `--scaffold=`.
    Scaffolds,
}

impl CompletionCandidates {
    pub fn run(self, launch: &LaunchConfig) -> eyre::Result<()> {
        if matches!(self, Self::Backends) {
            // A compiled enum: host-independent, so even a remote context
            // keeps this answer.
            print_candidates(backend_names());
            return Ok(());
        }
        if launch.ssh.is_some() || launch.mosh.is_some() {
            // Read-only completion never initiates a transport, and the remote
            // host's project root, tmux server and scaffold registry are not
            // observable from here. Say so instead of querying local state a
            // remote launch would never use.
            eprintln!(
                "term-preserver: completion inventories are local-only; no host inventory for {}",
                if launch.ssh.is_some() { "--ssh" } else { "--mosh" }
            );
            return Ok(());
        }
        match self {
            Self::Backends => unreachable!("handled above"),
            Self::Projects => print_candidates(project_names(launch)?),
            Self::Sessions => print_candidates(session_names(launch)?),
            Self::Scaffolds => print_candidates(scaffold_names(launch)?),
        }
        Ok(())
    }
}

/// Line-oriented protocol: sorted, deduplicated, no control characters (the
/// protocol cannot represent them in a line).
fn print_candidates(names: Vec<String>) {
    let mut names = names;
    names.sort();
    names.dedup();
    for name in names {
        if !name.chars().any(char::is_control) {
            println!("{name}");
        }
    }
}

/// Only backends this build compiled; `auto` is always offered.
fn backend_names() -> Vec<String> {
    SessionBackend::value_variants()
        .iter()
        .filter(|backend| backend.compiled())
        .map(|backend| {
            backend
                .to_possible_value()
                .expect("SessionBackend variants are all nameable")
                .get_name()
                .to_owned()
        })
        .collect()
}

/// Sorted immediate project basenames. Directory symlinks count; dangling
/// symlinks, regular files, deeper descendants, and non-UTF-8 names do not —
/// the same policy as tta's project inventory.
fn project_names(launch: &LaunchConfig) -> eyre::Result<Vec<String>> {
    let root = project_root(launch).ok_or_else(|| eyre::eyre!("cannot determine project root"))?;
    let entries = std::fs::read_dir(&root).map_err(|error| eyre::eyre!("cannot inspect project directory {}: {error}", root.display()))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Some(name) = entry.file_name().into_string().ok() else {
            continue;
        };
        let path = entry.path();
        let kind = entry.file_type()?;
        let is_directory = if kind.is_symlink() {
            match std::fs::metadata(&path) {
                Ok(metadata) => metadata.is_dir(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(eyre::eyre!("cannot inspect project directory {}: {error}", path.display())),
            }
        } else {
            kind.is_dir()
        };
        if is_directory {
            names.push(name);
        }
    }
    Ok(names)
}

/// Completion-shaped availability: the same auto order `select_backend` uses
/// for local launches, without target-action policy — a widget only needs the
/// inventory one backend would see. Explicit choices are validated honestly:
/// a backend this build did not compile is an error, never a silent fallback.
fn effective_backend(launch: &LaunchConfig) -> eyre::Result<SessionBackend> {
    match launch.session_backend {
        SessionBackend::Auto => {
            let selected = if cfg!(feature = "tta-lib") {
                SessionBackend::TtaLib
            } else if cfg!(feature = "tta-cli") && executable_present(&launch.tta_program) {
                SessionBackend::TtaCli
            } else if cfg!(feature = "classic-tmux-tmax") {
                SessionBackend::ClassicTmuxTmax
            } else if cfg!(feature = "classic-tmux-cli") {
                SessionBackend::ClassicTmuxCli
            } else {
                return Err(eyre::eyre!(
                    "no available session backend; enable a backend feature or install tta on the execution host"
                ));
            };
            Ok(selected)
        }
        backend => {
            if !backend.compiled() {
                eyre::bail!("session backend {backend:?} was not compiled");
            }
            Ok(backend)
        }
    }
}

fn session_names(launch: &LaunchConfig) -> eyre::Result<Vec<String>> {
    let socket = derive_socket(launch, &std::env::current_dir()?)?;
    match effective_backend(launch)? {
        #[cfg(feature = "tta-lib")]
        SessionBackend::TtaLib => embedded_sessions(launch, &socket),
        #[cfg(feature = "tta-cli")]
        SessionBackend::TtaCli => tta_cli_sessions(launch, &socket),
        #[cfg(feature = "classic-tmux-tmax")]
        SessionBackend::ClassicTmuxTmax => tmax_sessions(&socket),
        #[cfg(feature = "classic-tmux-cli")]
        SessionBackend::ClassicTmuxCli => cli_sessions(launch, &socket),
        backend => Err(eyre::eyre!("no compiled session backend can serve this query ({backend:?})")),
    }
}

/// The embedded backend picks its lookup exactly as `launcher::embedded_tta`
/// does: `TTA_BACKEND=cli` shells out, anything else native.
#[cfg(feature = "tta-lib")]
fn embedded_sessions(launch: &LaunchConfig, socket: &Path) -> eyre::Result<Vec<String>> {
    use tta_launcher::backend::{self, SessionLookup};
    let lookup: Box<dyn SessionLookup> = match std::env::var("TTA_BACKEND").as_deref() {
        Ok("cli") => Box::new(backend::Cli {
            program: launch.tmux_program.clone().into(),
            socket: socket.to_path_buf(),
        }),
        Err(std::env::VarError::NotPresent) | Ok("") | Ok("native") => Box::new(backend::Native {
            endpoint: tmax::endpoint::Tmux::new().socket_path(socket),
        }),
        _ => {
            eyre::bail!("TTA_BACKEND must be native or cli");
        }
    };
    Ok(lookup.sessions()?.into_iter().map(|session| session.name).collect())
}

/// Forward the endpoint to the installed tta's own candidate protocol.
#[cfg(feature = "tta-cli")]
fn tta_cli_sessions(launch: &LaunchConfig, socket: &Path) -> eyre::Result<Vec<String>> {
    let mut command = Command::new(&launch.tta_program);
    if let Some(root) = &launch.session_root {
        command.args(["--root", root]);
    }
    command
        .arg("--socket-path")
        .arg(socket)
        .arg("--tmux")
        .arg(&launch.tmux_program)
        .args(["--completion-candidates", "sessions"]);
    inventory_lines(command)
}

#[cfg(feature = "classic-tmux-tmax")]
fn tmax_sessions(socket: &Path) -> eyre::Result<Vec<String>> {
    use tmax::error::OperationError;
    let endpoint = tmax::endpoint::Tmux::new().socket_path(socket);
    let sessions = match endpoint.query(tmax::query::Sessions::new()) {
        Ok(sessions) => sessions,
        // An absent server or a live server with no sessions is an empty
        // inventory, not a failure (same classification as native discovery).
        Err(OperationError::ServerConnection { source, .. })
            if matches!(source.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) =>
        {
            return Ok(Vec::new())
        }
        Err(OperationError::Rejected(d)) if d.return_code == Some(1) && !d.truncated && d.body.is_empty() && d.stderr == b"no sessions\n" => {
            return Ok(Vec::new())
        }
        Err(error) => return Err(error.into()),
    };
    Ok(sessions.into_iter().map(|session| session.name).collect())
}

/// Classic CLI inventory with the same narrowly matched absence diagnostics as
/// `launcher::cli_existing`: a missing server yields no names and, crucially,
/// creates nothing.
#[cfg(feature = "classic-tmux-cli")]
fn cli_sessions(launch: &LaunchConfig, socket: &Path) -> eyre::Result<Vec<String>> {
    let output = Command::new(&launch.tmux_program)
        .arg("-u")
        .arg("-S")
        .arg(socket)
        .args(["list-sessions", "-F", "#{session_name}"])
        .env("LC_ALL", "C")
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.code() == Some(1)
            && output.stdout.is_empty()
            && (stderr == "no sessions\n"
                || stderr == format!("no server running on {}\n", socket.display())
                || stderr == format!("error connecting to {} (No such file or directory)\n", socket.display())
                || stderr == format!("error connecting to {} (Connection refused)\n", socket.display()))
        {
            return Ok(Vec::new());
        }
        eyre::bail!("tmux session inventory failed ({}): {stderr}", output.status);
    }
    Ok(String::from_utf8(output.stdout)?.lines().map(str::to_owned).collect())
}

/// The registry is tta's either way. The effective backend decides which
/// window to use: an explicit `tta-cli` asks the installed CLI even when the
/// embedded registry is compiled, so the inventory matches what that launch
/// would actually consult.
fn scaffold_names(launch: &LaunchConfig) -> eyre::Result<Vec<String>> {
    match effective_backend(launch) {
        Ok(SessionBackend::TtaCli) => cli_scaffold_names(launch),
        Ok(SessionBackend::TtaLib) => embedded_scaffold_names(),
        // Classic or unresolved: either registry window still answers;
        // prefer the in-process registry, else the CLI, else nothing.
        _ => {
            if cfg!(feature = "tta-lib") {
                embedded_scaffold_names()
            } else if cfg!(feature = "tta-cli") {
                cli_scaffold_names(launch)
            } else {
                Ok(Vec::new())
            }
        }
    }
}

#[cfg(feature = "tta-lib")]
fn embedded_scaffold_names() -> eyre::Result<Vec<String>> {
    let config = tta_launcher::scaffold::Config::load().map_err(tta_launcher::scaffold::Error::from)?;
    Ok(config.names())
}

#[cfg(not(feature = "tta-lib"))]
fn embedded_scaffold_names() -> eyre::Result<Vec<String>> {
    Ok(Vec::new())
}

#[cfg(feature = "tta-cli")]
fn cli_scaffold_names(launch: &LaunchConfig) -> eyre::Result<Vec<String>> {
    let mut query = Command::new(&launch.tta_program);
    query.args(["--completion-candidates", "scaffolds"]);
    inventory_lines(query)
}

#[cfg(not(feature = "tta-cli"))]
fn cli_scaffold_names(_launch: &LaunchConfig) -> eyre::Result<Vec<String>> {
    eyre::bail!("tta scaffold inventory requires the tta-cli feature")
}

#[cfg(feature = "tta-cli")]
fn inventory_lines(mut command: Command) -> eyre::Result<Vec<String>> {
    let output = command.output()?;
    if !output.status.success() {
        eyre::bail!(
            "tta candidate inventory failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?.lines().map(str::to_owned).collect())
}
