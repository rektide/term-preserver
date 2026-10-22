//! Read-only candidate inventories for shell completion, mirroring the tta
//! completion protocol. Queries observe state only: they never create, attach,
//! or switch a tmux session, never start a server (an absent server is an
//! empty session inventory), and never run a scaffold generator.
//!
//! Lookup configuration is the same merged [`LaunchConfig`] a launch uses, so
//! an inventory honors `--session-root`, `--tmux-socket`, program overrides,
//! and the ambient environment exactly as the real invocation would.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::ValueEnum;

use crate::launch::LaunchConfig;
use crate::launcher::SessionBackend;

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
        match self {
            Self::Backends => print_candidates(backend_names()),
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

/// Root precedence mirrors launching (`launcher::project_root`):
/// `--session-root` > `TTA_ROOT` > home-relative `PROJECTS_DIR` > `~/src`.
fn project_root(launch: &LaunchConfig) -> Option<PathBuf> {
    launch
        .session_root
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TTA_ROOT").filter(|s| !s.is_empty()).map(PathBuf::from))
        .or_else(|| {
            let projects = std::env::var_os("PROJECTS_DIR")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("src"));
            if projects.is_absolute() {
                Some(projects)
            } else {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(projects))
            }
        })
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

/// Socket precedence mirrors `launcher::normalize_local`: explicit
/// `--tmux-socket`, then `$TMUX`'s server, then the conventional default.
/// Relative paths are frozen against the invocation cwd.
fn local_socket(launch: &LaunchConfig) -> eyre::Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let socket = if let Some(socket) = launch.tmux_socket.as_deref() {
        if socket.is_empty() {
            eyre::bail!("tmux socket path must not be empty");
        }
        cwd.join(socket)
    } else if let Some(socket) = std::env::var_os("TMUX")
        .and_then(|s| s.into_string().ok())
        .and_then(|s| s.split(',').next().filter(|s| !s.is_empty()).map(str::to_owned))
    {
        cwd.join(socket)
    } else {
        // An explicit path avoids client-specific TMUX_TMPDIR behavior.
        let base = std::env::var_os("TMUX_TMPDIR")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        cwd.join(base).join(format!("tmux-{}", nix::unistd::geteuid().as_raw())).join("default")
    };
    socket
        .into_os_string()
        .into_string()
        .map(PathBuf::from)
        .map_err(|_| eyre::eyre!("tmux socket path must be UTF-8"))
}

/// Mirrors the private `launcher::executable_present` availability probe.
fn executable_present(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &Path| p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
    if program.contains('/') {
        return executable(Path::new(program));
    }
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| executable(&dir.join(program))))
}

/// Completion-shaped availability: the same auto order `select_backend` uses
/// for local launches, without target or transport policy (a widget only
/// needs the inventory one backend would see).
fn effective_backend(launch: &LaunchConfig) -> SessionBackend {
    match launch.session_backend {
        SessionBackend::Auto => {
            if cfg!(feature = "tta-lib") {
                SessionBackend::TtaLib
            } else if cfg!(feature = "tta-cli") && executable_present(&launch.tta_program) {
                SessionBackend::TtaCli
            } else if cfg!(feature = "classic-tmux-tmax") {
                SessionBackend::ClassicTmuxTmax
            } else {
                SessionBackend::ClassicTmuxCli
            }
        }
        backend => backend,
    }
}

fn session_names(launch: &LaunchConfig) -> eyre::Result<Vec<String>> {
    let socket = local_socket(launch)?;
    match effective_backend(launch) {
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

fn scaffold_names(launch: &LaunchConfig) -> eyre::Result<Vec<String>> {
    // The registry is tta's either way: load it in-process when embedded,
    // otherwise ask the installed CLI through the same candidate protocol.
    #[cfg(feature = "tta-lib")]
    {
        let _ = launch;
        let config = tta_launcher::scaffold::Config::load().map_err(tta_launcher::scaffold::Error::from)?;
        Ok(config.names())
    }
    #[cfg(all(feature = "tta-cli", not(feature = "tta-lib")))]
    {
        let mut query = Command::new(&launch.tta_program);
        query.args(["--completion-candidates", "scaffolds"]);
        inventory_lines(query)
    }
    #[cfg(not(any(feature = "tta-lib", feature = "tta-cli")))]
    {
        let _ = launch;
        // No tta implementation is compiled: there is no registry vocabulary
        // to offer, and launching would reject --scaffold anyway.
        Ok(Vec::new())
    }
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
