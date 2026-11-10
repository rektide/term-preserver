//! Availability policy and per-attempt session launching. The worker runs inside
//! the child PTY, after the selected shell's startup files, then execs the client.
//! Retrying therefore repeats discovery rather than reusing a stale session ID.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[cfg(feature = "classic-tmux")]
use crate::launch::TmuxMode;
use crate::launch::{shell_join, shell_quote, ssh_control_options, wrap_transport, LaunchConfig};
use crate::session::CommandSpec;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum SessionBackend {
    #[default]
    Auto,
    TtaLib,
    TtaCli,
    ClassicTmuxCli,
    ClassicTmuxTmax,
}

impl SessionBackend {
    pub fn compiled(self) -> bool {
        match self {
            Self::Auto => true,
            Self::TtaLib => cfg!(feature = "tta-lib"),
            Self::TtaCli => cfg!(feature = "tta-cli"),
            Self::ClassicTmuxCli => cfg!(feature = "classic-tmux-cli"),
            Self::ClassicTmuxTmax => cfg!(feature = "classic-tmux-tmax"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Request {
    launch: LaunchConfig,
    trailing: Vec<String>,
    /// Computed once by the parent. The local attempt worker uses this same
    /// connection/session policy for every non-PTY probe and final PTY handoff.
    #[serde(default)]
    ssh_options: Option<Vec<String>>,
}

/// Freeze the local socket for discovery, handoff, and observation alike.
/// Remote socket paths belong to the remote host and must not be normalized here.
pub fn normalize_local(launch: &mut LaunchConfig) -> eyre::Result<()> {
    if launch.tmux_mode().is_none() || launch.ssh.is_some() || launch.mosh.is_some() {
        return Ok(());
    }
    let socket = derive_socket(launch, &std::env::current_dir()?)?;
    launch.tmux_socket = Some(
        socket
            .into_os_string()
            .into_string()
            .map_err(|_| eyre::eyre!("tmux socket path must be UTF-8"))?,
    );
    Ok(())
}

/// The endpoint a local launch — or a local read-only query such as
/// completion — would use: explicit `--tmux-socket` frozen against `cwd`,
/// then `$TMUX`'s server, then the conventional default. Shared so inventory
/// and launch can never disagree about which server they mean.
pub(crate) fn derive_socket(launch: &LaunchConfig, cwd: &Path) -> eyre::Result<PathBuf> {
    let socket = if let Some(socket) = &launch.tmux_socket {
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
    Ok(socket)
}

/// Pure backend policy once executable presence has been observed. Availability
/// fallback is confined to Auto; resolution/protocol/permission errors never
/// select another implementation.
pub fn select_backend(launch: &LaunchConfig, tta_present: bool) -> eyre::Result<SessionBackend> {
    let requested = launch.session_backend;
    let classic_only = launch.tmux_create.is_some() || launch.tmux_attach.is_some();
    let requires_tta = launch.tta.is_some()
        || launch
            .scaffold
            .as_deref()
            .is_some_and(|choice| !matches!(choice, "false" | "0" | "off" | "none"));
    if classic_only && requires_tta {
        eyre::bail!("--scaffold requires a tta create-or-match target (--tmux or --tta)");
    }
    if requested != SessionBackend::Auto {
        if !requested.compiled() {
            eyre::bail!("session backend {requested:?} was not compiled");
        }
        if classic_only && matches!(requested, SessionBackend::TtaLib | SessionBackend::TtaCli) {
            eyre::bail!("--tmux-create/--tmux-attach require a classic tmux backend");
        }
        if requires_tta && matches!(requested, SessionBackend::ClassicTmuxCli | SessionBackend::ClassicTmuxTmax) {
            eyre::bail!("--tta or enabled --scaffold requires a tta backend");
        }
        return Ok(requested);
    }
    let remote = launch.ssh.is_some() || launch.mosh.is_some();
    if !classic_only {
        if cfg!(feature = "tta-lib") && (!remote || launch.ssh.is_some()) {
            return Ok(SessionBackend::TtaLib);
        }
        if cfg!(feature = "tta-cli") && tta_present {
            return Ok(SessionBackend::TtaCli);
        }
    }
    if !requires_tta {
        if cfg!(feature = "classic-tmux-tmax") && !remote {
            return Ok(SessionBackend::ClassicTmuxTmax);
        }
        if cfg!(feature = "classic-tmux-cli") {
            return Ok(SessionBackend::ClassicTmuxCli);
        }
    }
    eyre::bail!("no available session backend; enable a backend feature or install tta on the execution host");
}

/// Prepare a repeatable command without querying or modifying any tmux server.
pub fn prepare(launch: &LaunchConfig, trailing: &[String], env_shell: Option<&str>) -> eyre::Result<CommandSpec> {
    launch.validate()?;
    if launch.tmux_mode().is_none() {
        return launch.resolve_shell_command(trailing, env_shell);
    }
    // Validate feature/operation compatibility here; executable presence is
    // resolved later, in the child's login environment or on the remote host.
    let backend = select_backend(launch, true)?;
    // Embedded SSH planning must run in a LOCAL attempt worker. Sending the
    // worker (or installed target tta) through the remote carrier would recreate
    // the dependency this path is meant to remove. Shell startup happens later
    // on the target, identically for probes and the prepared action.
    if launch.ssh.is_some() && backend == SessionBackend::TtaLib {
        return CommandSpec::try_from(worker_argv(launch.clone(), trailing, Some(ssh_control_options(launch)))?);
    }
    let remote = launch.ssh.is_some() || launch.mosh.is_some();
    let command = if remote {
        remote_command(launch, trailing)?
    } else {
        let mut local = launch.clone();
        normalize_local(&mut local)?;
        worker_argv(local, trailing, None)?
    };
    // The command-language carrier is always POSIX sh. The outer selected shell
    // performs login/environment setup, then hands it literal argv (also in nu).
    let script = shell_join(&command);
    let shell = launch.shell.as_deref().or(env_shell).filter(|s| !s.is_empty()).unwrap_or("/bin/sh");
    let family = Path::new(shell).file_name().and_then(|s| s.to_str()).unwrap_or(shell);
    let mut argv = vec![shell.to_owned()];
    if !matches!(family, "nu" | "nushell") {
        if launch.login {
            argv.push("-l".into());
        }
        if launch.interactive {
            argv.push("-i".into());
        }
    }
    argv.push("-c".into());
    // POSIX quoting is carried by sh, not reinterpreted by fish/nu. Fish accepts
    // single-quoted tokens but has different single-quote escaping, so encode it.
    let script = if family == "fish" {
        command
            .iter()
            .map(|s| format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")))
            .collect::<Vec<_>>()
            .join(" ")
    } else if matches!(family, "nu" | "nushell") {
        format!("^{}", command.iter().map(|s| nu_quote(s)).collect::<Vec<_>>().join(" "))
    } else {
        format!("exec {script}")
    };
    argv.push(if family == "fish" { format!("exec {script}") } else { script });
    CommandSpec::try_from(wrap_transport(
        argv,
        launch.ssh.as_deref(),
        launch.mosh.as_deref(),
        &ssh_control_options(launch),
    ))
}

fn worker_argv(launch: LaunchConfig, trailing: &[String], ssh_options: Option<Vec<String>>) -> eyre::Result<Vec<String>> {
    Ok(vec![
        std::env::current_exe()?
            .into_os_string()
            .into_string()
            .map_err(|_| eyre::eyre!("executable path must be UTF-8"))?,
        "--launch-worker".into(),
        serde_json::to_string(&Request {
            launch,
            trailing: trailing.to_vec(),
            ssh_options,
        })?,
    ])
}

// Nu double-quoted strings are literal (interpolation requires a leading $).
// Unicode escapes use \u{HEX}, not JSON's \uHHHH form:
// https://www.nushell.sh/book/working_with_strings.html#double-quoted-strings
fn nu_quote(value: &str) -> String {
    use std::fmt::Write;
    let mut quoted = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            ch if ch.is_control() => write!(&mut quoted, "\\u{{{:x}}}", ch as u32).unwrap(),
            ch => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

fn tta_argv(launch: &LaunchConfig, trailing: &[String]) -> Vec<String> {
    let mut args = vec![launch.tta_program.clone()];
    if let Some(root) = &launch.session_root {
        args.extend(["--root".into(), root.clone()]);
    }
    if let Some(socket) = &launch.tmux_socket {
        args.extend(["--socket-path".into(), socket.clone()]);
    }
    args.extend(["--tmux".into(), launch.tmux_program.clone()]);
    if let Some(choice) = &launch.scaffold {
        args.push(format!("--scaffold={choice}"));
    }
    if launch.tmux_group {
        args.push("--group".into());
    }
    if let Some(dir) = &launch.dir {
        args.extend(["--dir".into(), dir.clone()]);
    }
    // Both the fragment and startup arguments follow --, protecting even a
    // leading '-' fragment from option parsing.
    args.push("--".into());
    args.push(launch.tmux_mode().expect("session target").session().to_owned());
    args.extend(trailing.iter().cloned());
    args
}

fn tta_remote_argv(launch: &LaunchConfig, trailing: &[String]) -> Vec<String> {
    let argv = tta_argv(launch, trailing);
    if launch.scaffold.is_some() {
        return argv;
    }
    // Resolve the default in the execution host's environment, independently of
    // the installed CLI's scaffold-by-default build feature.
    vec![
        "sh".into(),
        "-c".into(),
        format!(
            "exec {} --scaffold=\"${{TTA_SCAFFOLD:-default}}\" {}",
            shell_quote(&argv[0]),
            shell_join(&argv[1..])
        ),
    ]
}

fn scaffold_choice(launch: &LaunchConfig) -> eyre::Result<String> {
    if let Some(choice) = &launch.scaffold {
        return Ok(choice.clone());
    }
    match std::env::var("TTA_SCAFFOLD") {
        Ok(value) if !value.is_empty() => Ok(value),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok("default".into()),
        Err(error) => Err(error.into()),
    }
}

fn remote_command(launch: &LaunchConfig, trailing: &[String]) -> eyre::Result<Vec<String>> {
    use SessionBackend::*;
    if matches!(launch.session_backend, TtaLib | ClassicTmuxTmax) {
        eyre::bail!("embedded/native launch backends require a local server; use tta-cli or classic-tmux-cli for ssh/mosh");
    }
    let fallback = select_backend(launch, false);
    let tta_allowed =
        cfg!(feature = "tta-cli") && launch.tmux_create.is_none() && launch.tmux_attach.is_none() && matches!(launch.session_backend, Auto | TtaCli);
    if launch.session_backend == TtaCli {
        select_backend(launch, true)?;
        return Ok(tta_remote_argv(launch, trailing));
    }
    let fallback_script = match fallback {
        Ok(ClassicTmuxCli) => classic_remote_script(launch, trailing)?,
        Err(_) if tta_allowed => "printf '%s\\n' 'term-preserver: tta is unavailable on this host and no classic fallback is enabled' >&2; exit 127".into(),
        Err(error) => return Err(error),
        _ => unreachable!(),
    };
    let script = if tta_allowed {
        format!(
            "if command -v {} >/dev/null 2>&1; then exec {}; else {}; fi",
            shell_quote(&launch.tta_program),
            shell_join(&tta_remote_argv(launch, trailing)),
            fallback_script
        )
    } else {
        fallback_script
    };
    Ok(vec!["sh".into(), "-c".into(), script])
}

#[cfg(feature = "classic-tmux-cli")]
fn classic_remote_script(launch: &LaunchConfig, trailing: &[String]) -> eyre::Result<String> {
    let mode = launch.tmux_mode().expect("session target");
    validate_name(mode.session())?;
    let base = tmux_argv(launch);
    let primary = classic_argv(launch, trailing, None)?;
    if matches!(mode, TmuxMode::CreateOrAttach(_)) && launch.tmux_group {
        let mut probe = base;
        probe.extend(["has-session".into(), "-t".into(), tmux_word(&format!("={}", mode.session()))]);
        let satellite = classic_argv(launch, trailing, Some(&format!("={}", mode.session())))?;
        Ok(format!(
            "if {} 2>/dev/null; then exec {}; else exec {}; fi",
            shell_join(&probe),
            shell_join(&satellite),
            shell_join(&primary)
        ))
    } else {
        Ok(format!("exec {}", shell_join(&primary)))
    }
}

#[cfg(not(feature = "classic-tmux-cli"))]
fn classic_remote_script(_: &LaunchConfig, _: &[String]) -> eyre::Result<String> {
    eyre::bail!("classic tmux CLI backend was not compiled");
}

/// Shared availability probe: completion asks the same question the launch
/// worker answers here.
pub(crate) fn executable_present(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &Path| p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
    if program.contains('/') {
        return executable(Path::new(program));
    }
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| executable(&dir.join(program))))
}

/// Entry point used only by the PTY child; no parent terminal is replaced.
pub fn execute_worker(json: &str) -> eyre::Result<()> {
    let Request {
        mut launch,
        trailing,
        ssh_options,
    } = serde_json::from_str(json)?;
    launch.validate()?;
    let backend = select_backend(&launch, executable_present(&launch.tta_program))?;
    #[cfg(feature = "tta-lib")]
    if launch.ssh.is_some() && backend == SessionBackend::TtaLib {
        let options = ssh_options.unwrap_or_else(|| ssh_control_options(&launch));
        return embedded_ssh(&launch, &trailing, options);
    }
    #[cfg(not(feature = "tta-lib"))]
    let _ = ssh_options;
    if matches!(backend, SessionBackend::TtaLib | SessionBackend::TtaCli) {
        launch.scaffold = Some(scaffold_choice(&launch)?);
    }
    // Normalize even for direct library callers of the worker entrypoint.
    normalize_local(&mut launch)?;
    ensure_default_socket_directory(&launch)?;
    match backend {
        SessionBackend::TtaCli => exec_argv(tta_argv(&launch, &trailing)),
        #[cfg(feature = "tta-lib")]
        SessionBackend::TtaLib => embedded_tta(&launch, &trailing),
        #[cfg(any(feature = "classic-tmux-cli", feature = "classic-tmux-tmax"))]
        SessionBackend::ClassicTmuxCli | SessionBackend::ClassicTmuxTmax => classic_local(&launch, &trailing, backend),
        _ => Err(eyre::eyre!("session backend {backend:?} was not compiled")),
    }
}

#[cfg(feature = "tta-lib")]
fn embedded_ssh(launch: &LaunchConfig, trailing: &[String], ssh_options: Vec<String>) -> eyre::Result<()> {
    use tta_command::Handoff;
    use tta_launcher::entry::{
        EntryAttempt, context,
        protocol::{AttemptOutcome, EntryIntent, ViewPreference, Waiting},
    };
    let destination = launch.ssh.clone().expect("SSH worker");
    let ssh = tta_runner::ssh::Ssh::builder()
        .destination(destination.clone())
        .options(ssh_options.into_iter().map(Into::into).collect())
        .build();
    let setup = tta_launcher::target::TargetSetup::builder()
        .maybe_shell(launch.shell.clone())
        .login(launch.login)
        .interactive(launch.interactive)
        .build();
    let cwd = std::env::current_dir()?;
    let mut attempt = EntryAttempt::acquire(context::Acquisition {
        scope: context::Scope {
            origin: context::Origin::Ssh { host: destination, setup },
            cwd: None,
        },
        // Only explicit target-side choices; target-local defaults come from
        // target facts in the shared context policy, never local expansion.
        overrides: context::Overrides {
            roots: launch.session_root.as_ref().map(|root| vec![cwd.join(root)]),
            socket: launch
                .tmux_socket
                .as_ref()
                .map(|path| tta_launcher::socket::Selection::Path(cwd.join(path))),
            program: Some(launch.tmux_program.clone().into()),
            scaffold: launch.scaffold.clone(),
            ..Default::default()
        },
        // Explicit grouping maps to Independent/Shared, never Auto.
        intent: EntryIntent {
            selector: launch.tmux_mode().expect("session target").session().to_owned(),
            view: if launch.tmux_group { ViewPreference::Independent } else { ViewPreference::Shared },
            directory: launch.dir.as_ref().map(|dir| cwd.join(dir)),
            view_directory: launch.dir.as_ref().map(|dir| cwd.join(dir)),
            startup: trailing.iter().map(Into::into).collect(),
            ..EntryIntent::selector("replaced-by-selector-above")
        },
        default_scaffold: "default".into(),
    })
    .map_err(|error| eyre::eyre!("invalid entry invocation: {error}"))?;
    let scope = match attempt.waiting() {
        Waiting::Read(request) => (*request.scope).clone(),
        _ => eyre::bail!("acquisition must start with a context read"),
    };
    let limits = tta_runner::capture::Limits {
        timeout: std::time::Duration::from_millis(launch.ssh_probe_timeout.get()),
        per_stream: launch.ssh_probe_retention.get(),
        ..Default::default()
    };
    let mut observer = tta_runner::observe::ssh::Ssh::acquire(ssh, scope, limits).map_err(|error| eyre::eyre!("{error:#}"))?;
    // One terminal effect dispatched through the same frozen connection and
    // setup that answered every read. Proxy/retry policy stays owned here; a
    // user retry builds a fresh attempt with fresh observations.
    let snapshot = tta_runner::execute::ssh::drive(&mut attempt, &mut observer, false, Handoff::Exec)
        .map_err(|error| eyre::eyre!("SSH entry attempt failed: {error:#}"))?;
    // Keep probe diagnostics off the framed stdout and visible in the PTY log.
    tta_runner::observe::ssh::write_diagnostics(std::io::stderr().lock(), &observer.observations)?;
    match snapshot.outcome {
        Some(AttemptOutcome::Stopped { cause, .. }) => Err(eyre::eyre!("SSH planning stopped before mutation: {cause:#}")),
        Some(AttemptOutcome::Cancelled { boundary, .. }) => Err(eyre::eyre!("SSH entry attempt cancelled at {boundary}")),
        None => eyre::bail!("SSH attempt ended without an outcome at {}", snapshot.leaf),
        // A completed terminal effect under exec cannot happen (exec never
        // returns on success); reaching here means the block ran to a status.
        Some(AttemptOutcome::Completed { effects }) => std::process::exit(
            effects
                .finished
                .last()
                .and_then(|(_, receipt)| match receipt {
                    tta_launcher::entry::protocol::EffectReceipt::Terminal { code } => *code,
                    _ => None,
                })
                .map_or(0, |code| code.clamp(0, 255)),
        ),
        Some(AttemptOutcome::Delegated { .. }) | Some(AttemptOutcome::Exported(_)) => std::process::exit(0),
    }
}

// Explicit -S freezes endpoint identity but unlike -L does not create tmux's
// default private directory on first use. Only create that conventional directory;
// arbitrary user-selected socket parents remain the caller's responsibility.
fn ensure_default_socket_directory(launch: &LaunchConfig) -> eyre::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let base = std::env::var_os("TMUX_TMPDIR")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let directory = std::env::current_dir()?.join(base).join(format!("tmux-{}", nix::unistd::geteuid().as_raw()));
    if launch.tmux_socket.as_deref().map(Path::new) == Some(directory.join("default").as_path()) {
        match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && directory.is_dir() => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn exec_argv(argv: Vec<String>) -> eyre::Result<()> {
    let (program, args) = argv.split_first().ok_or_else(|| eyre::eyre!("empty client command"))?;
    Err(Command::new(program).args(args).exec().into())
}

#[cfg(feature = "tta-lib")]
fn embedded_tta(launch: &LaunchConfig, trailing: &[String]) -> eyre::Result<()> {
    use tta_command::Handoff;
    use tta_launcher::entry::{
        EntryAttempt, context,
        protocol::{AttemptOutcome, Delivery, EntryIntent, ViewPreference, Waiting},
    };
    let cwd = std::env::current_dir()?;
    let native = match std::env::var("TTA_BACKEND").as_deref() {
        Ok("cli") => false,
        Err(std::env::VarError::NotPresent) | Ok("") | Ok("native") => true,
        _ => eyre::bail!("TTA_BACKEND must be native or cli"),
    };
    let program = if launch.tmux_program.contains('/') {
        cwd.join(&launch.tmux_program).into_os_string()
    } else {
        launch.tmux_program.clone().into()
    };
    let config = std::env::var_os("TTA_TMUX_CONFIG").filter(|s| !s.is_empty()).map(|p| cwd.join(p));
    let mut attempt = EntryAttempt::acquire(context::Acquisition {
        scope: context::Scope {
            origin: context::Origin::Local { host: "local".into() },
            cwd: None,
        },
        overrides: context::Overrides {
            // The consumer keeps its own independent ordered-roots policy and
            // supplies the resolved list explicitly.
            roots: Some(project_roots(launch)?),
            socket: Some(tta_launcher::socket::Selection::Path(
                launch.tmux_socket.as_ref().expect("normalized socket").into(),
            )),
            program: Some(program),
            config,
            scaffold: Some(scaffold_choice(launch)?),
            ..Default::default()
        },
        // Explicit grouping maps to Independent/Shared, never Auto.
        intent: EntryIntent {
            selector: launch.tmux_mode().expect("session target").session().to_owned(),
            view: if launch.tmux_group { ViewPreference::Independent } else { ViewPreference::Shared },
            directory: launch.dir.as_ref().map(|dir| cwd.join(dir)),
            view_directory: launch.dir.as_ref().map(|dir| cwd.join(dir)),
            startup: trailing.iter().map(Into::into).collect(),
            ..EntryIntent::selector("replaced-by-selector-above")
        },
        default_scaffold: "default".into(),
    })
    .map_err(|error| eyre::eyre!("invalid entry invocation: {error}"))?;
    let scope = match attempt.waiting() {
        Waiting::Read(request) => (*request.scope).clone(),
        _ => eyre::bail!("acquisition must start with a context read"),
    };
    let mut sessions = if native {
        tta_runner::observe::local::SessionBackend::Native
    } else {
        tta_runner::observe::local::SessionBackend::Cli
    };
    let mut observer = tta_runner::observe::local::Local::acquire(scope, &mut sessions).map_err(|error| eyre::eyre!("{error:#}"))?;
    let mut executor = tta_runner::execute::local::Local::default();
    // The worker's final exec replaces this process with the real client; a
    // user retry runs this worker again, building a fresh attempt.
    let snapshot = tta_runner::execute::drive(
        &mut attempt,
        &mut observer,
        &mut executor,
        Delivery::Execute { native },
        Handoff::Exec,
    )
    .map_err(|error| eyre::eyre!("entry attempt failed: {error:#}"))?;
    match snapshot.outcome {
        Some(AttemptOutcome::Stopped { cause, .. }) => Err(eyre::eyre!("tmux launch failed; do not replay a possibly executed mutation: {cause:#}")),
        Some(AttemptOutcome::Cancelled { boundary, .. }) => Err(eyre::eyre!("entry attempt cancelled at {boundary}")),
        None => eyre::bail!("entry attempt ended without an outcome at {}", snapshot.leaf),
        Some(AttemptOutcome::Completed { effects }) => std::process::exit(
            effects
                .finished
                .last()
                .and_then(|(_, receipt)| match receipt {
                    tta_launcher::entry::protocol::EffectReceipt::Terminal { code } => *code,
                    _ => None,
                })
                .map_or(0, |code| code.clamp(0, 255)),
        ),
        Some(AttemptOutcome::Delegated { .. }) | Some(AttemptOutcome::Exported(_)) => std::process::exit(0),
    }
}

/// Ordered project roots for tta project matching: `--session-root`, then
/// `TTA_ROOTS` (a JSON array of nonempty paths), then `TTA_ROOT`, then
/// home-relative `PROJECTS_DIR`, then `~/src`. One active layer replaces the
/// whole list, mirroring tta's declared configuration precedence; relative
/// entries freeze against the invocation cwd. Shared with completion so
/// inventories and launches see the same projects.
pub(crate) fn project_roots(launch: &LaunchConfig) -> eyre::Result<Vec<PathBuf>> {
    let value = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    let cwd = std::env::current_dir()?;
    let roots = if let Some(root) = &launch.session_root {
        vec![PathBuf::from(root)]
    } else if let Some(json) = value("TTA_ROOTS") {
        let text = json.into_string().map_err(|_| eyre::eyre!("TTA_ROOTS must be a UTF-8 JSON array of paths"))?;
        let paths: Vec<PathBuf> = serde_json::from_str(&text).map_err(|error| eyre::eyre!("TTA_ROOTS must be a JSON array of nonempty paths: {error}"))?;
        if paths.is_empty() || paths.iter().any(|path| path.as_os_str().is_empty()) {
            eyre::bail!("TTA_ROOTS must list at least one nonempty path");
        }
        paths
    } else if let Some(root) = value("TTA_ROOT") {
        vec![PathBuf::from(root)]
    } else {
        let projects = value("PROJECTS_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("src"));
        if projects.is_absolute() {
            vec![projects]
        } else {
            value("HOME").map(|home| vec![PathBuf::from(home).join(projects)]).unwrap_or_default()
        }
    };
    Ok(roots.into_iter().map(|root| cwd.join(root)).collect())
}

#[cfg(feature = "classic-tmux")]
fn validate_name(name: &str) -> eyre::Result<()> {
    if name.is_empty() || name.contains(['.', ':']) || name.chars().any(char::is_control) {
        eyre::bail!("tmux session name must be nonempty and contain no '.', ':', or control characters");
    }
    Ok(())
}

#[cfg(feature = "classic-tmux")]
fn tmux_argv(launch: &LaunchConfig) -> Vec<String> {
    let mut argv = vec![launch.tmux_program.clone()];
    if let Some(socket) = &launch.tmux_socket {
        argv.extend(["-S".into(), socket.clone()]);
    }
    argv
}

#[cfg(feature = "classic-tmux")]
fn tmux_word(word: &str) -> String {
    match word.strip_suffix(';') {
        Some(prefix) => format!("{prefix}\\;"),
        None => word.to_owned(),
    }
}

#[cfg(feature = "classic-tmux")]
fn classic_argv(launch: &LaunchConfig, trailing: &[String], existing: Option<&str>) -> eyre::Result<Vec<String>> {
    let mode = launch.tmux_mode().expect("session target");
    validate_name(mode.session())?;
    let mut argv = tmux_argv(launch);
    let command_start = argv.len();
    match &mode {
        TmuxMode::Attach(name) => argv.extend(["attach-session".into(), "-t".into(), format!("={name}")]),
        _ if existing.is_some() && launch.tmux_group => {
            argv.extend(["new-session".into(), "-t".into(), existing.unwrap().into()]);
        }
        _ => {
            argv.push("new-session".into());
            if matches!(mode, TmuxMode::CreateOrAttach(_)) {
                argv.push("-A".into());
            }
            argv.extend(["-s".into(), mode.session().replace('#', "#{a:35}")]);
        }
    }
    if let Some(dir) = &launch.dir {
        argv.extend(["-c".into(), dir.replace('#', "#{a:35}")]);
    }
    if existing.is_some() && launch.tmux_group {
        argv.extend([";".into(), "set-option".into(), "destroy-unattached".into(), "keep-last".into()]);
    } else if !matches!(mode, TmuxMode::Attach(_)) && !trailing.is_empty() {
        // tmux takes a single startup argument as shell text. Supplying a shell
        // plus quoted command string preserves even a one-element literal argv.
        argv.extend(["--".into(), "sh".into(), "-c".into(), format!("exec {}", shell_join(trailing))]);
    }
    // tmux itself interprets semicolons even when they arrive in literal argv.
    // Only the intentional command separator above remains unescaped.
    let separator = if existing.is_some() && launch.tmux_group {
        Some(argv.len() - 4)
    } else {
        None
    };
    for (index, arg) in argv.iter_mut().enumerate().skip(command_start) {
        if Some(index) != separator {
            *arg = tmux_word(arg);
        }
    }
    Ok(argv)
}

#[cfg(any(feature = "classic-tmux-cli", feature = "classic-tmux-tmax"))]
fn classic_local(launch: &LaunchConfig, trailing: &[String], backend: SessionBackend) -> eyre::Result<()> {
    let mode = launch.tmux_mode().expect("session target");
    let existing = if matches!(mode, TmuxMode::CreateOrAttach(_)) && launch.tmux_group {
        match backend {
            #[cfg(feature = "classic-tmux-tmax")]
            SessionBackend::ClassicTmuxTmax => native_existing(launch, mode.session())?,
            #[cfg(feature = "classic-tmux-cli")]
            SessionBackend::ClassicTmuxCli => cli_existing(launch, mode.session())?,
            _ => {
                eyre::bail!("classic backend was not compiled");
            }
        }
    } else {
        None
    };
    exec_argv(classic_argv(launch, trailing, existing.as_deref())?)
}

#[cfg(feature = "classic-tmux-cli")]
fn cli_existing(launch: &LaunchConfig, name: &str) -> eyre::Result<Option<String>> {
    let argv = tmux_argv(launch);
    let output = Command::new(&argv[0])
        .arg("-u")
        .args(&argv[1..])
        .args(["list-sessions", "-F", "#{session_id}\t#{session_name}"])
        .env("LC_ALL", "C")
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let path = launch.tmux_socket.as_deref().unwrap();
        if output.status.code() == Some(1)
            && output.stdout.is_empty()
            && (stderr == "no sessions\n"
                || stderr == format!("no server running on {path}\n")
                || stderr == format!("error connecting to {path} (No such file or directory)\n")
                || stderr == format!("error connecting to {path} (Connection refused)\n"))
        {
            return Ok(None);
        }
        eyre::bail!("tmux session lookup failed ({}): {stderr}", output.status);
    }
    Ok(std::str::from_utf8(&output.stdout)?
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .find(|(_, n)| *n == name)
        .map(|(id, _)| id.to_owned()))
}

#[cfg(feature = "classic-tmux-tmax")]
fn native_existing(launch: &LaunchConfig, name: &str) -> eyre::Result<Option<String>> {
    use tmax::error::OperationError;
    let endpoint = tmax::endpoint::Tmux::new().socket_path(launch.tmux_socket.as_ref().unwrap());
    let sessions = match endpoint.query(tmax::query::Sessions::new()) {
        Ok(sessions) => sessions,
        Err(OperationError::ServerConnection { source, .. })
            if matches!(source.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) =>
        {
            return Ok(None)
        }
        Err(OperationError::Rejected(d)) if d.return_code == Some(1) && !d.truncated && d.body.is_empty() && d.stderr == b"no sessions\n" => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    Ok(sessions.into_iter().find(|s| s.name == name).map(|s| s.session_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> LaunchConfig {
        LaunchConfig {
            tmux: Some("dev".into()),
            ..Default::default()
        }
    }

    #[test]
    fn availability_prefers_embedded_ssh_but_keeps_mosh_on_existing_carriers() {
        let launch = target();
        let expected = if cfg!(feature = "tta-lib") {
            Some(SessionBackend::TtaLib)
        } else if cfg!(feature = "tta-cli") {
            Some(SessionBackend::TtaCli)
        } else if cfg!(feature = "classic-tmux-tmax") {
            Some(SessionBackend::ClassicTmuxTmax)
        } else if cfg!(feature = "classic-tmux-cli") {
            Some(SessionBackend::ClassicTmuxCli)
        } else {
            None
        };
        assert_eq!(select_backend(&launch, true).ok(), expected);
        let remote = LaunchConfig {
            ssh: Some("w".into()),
            ..launch
        };
        let expected = if cfg!(feature = "tta-lib") {
            Some(SessionBackend::TtaLib)
        } else if cfg!(feature = "tta-cli") {
            Some(SessionBackend::TtaCli)
        } else if cfg!(feature = "classic-tmux-cli") {
            Some(SessionBackend::ClassicTmuxCli)
        } else {
            None
        };
        assert_eq!(select_backend(&remote, true).ok(), expected);
        assert_eq!(
            select_backend(&remote, false).ok(),
            if cfg!(feature = "tta-lib") {
                Some(SessionBackend::TtaLib)
            } else {
                cfg!(feature = "classic-tmux-cli").then_some(SessionBackend::ClassicTmuxCli)
            }
        );
        let mosh = LaunchConfig {
            ssh: None,
            mosh: Some("w".into()),
            ..remote
        };
        let expected = if cfg!(feature = "tta-cli") {
            Some(SessionBackend::TtaCli)
        } else {
            cfg!(feature = "classic-tmux-cli").then_some(SessionBackend::ClassicTmuxCli)
        };
        assert_eq!(select_backend(&mosh, true).ok(), expected);
    }

    #[cfg(feature = "tta-lib")]
    #[test]
    fn embedded_ssh_worker_carries_one_option_set_and_target_shell_without_local_setup() {
        let launch = LaunchConfig {
            ssh: Some("host".into()),
            shell: Some("/target/shell".into()),
            session_root: Some("relative-target-root".into()),
            tmux_socket: Some("relative-target-socket".into()),
            ssh_master: false,
            ..target()
        };
        let spec = prepare(&launch, &["literal ' $;".into()], Some("/local/wrong-shell")).unwrap();
        assert_eq!(spec.args[0], "--launch-worker");
        let request: Request = serde_json::from_str(&spec.args[1]).unwrap();
        assert_eq!(request.ssh_options, Some(Vec::new()));
        assert_eq!(request.launch.shell.as_deref(), Some("/target/shell"));
        assert_eq!(request.launch.session_root.as_deref(), Some("relative-target-root"));
        assert_eq!(request.launch.tmux_socket.as_deref(), Some("relative-target-socket"));
        assert_eq!(request.trailing, ["literal ' $;"]);
    }

    #[test]
    fn explicit_backend_never_falls_back_when_not_compiled() {
        for backend in [
            SessionBackend::TtaLib,
            SessionBackend::TtaCli,
            SessionBackend::ClassicTmuxCli,
            SessionBackend::ClassicTmuxTmax,
        ] {
            let launch = LaunchConfig {
                session_backend: backend,
                ..target()
            };
            assert_eq!(select_backend(&launch, false).ok(), backend.compiled().then_some(backend));
        }
    }

    #[test]
    fn explicit_tta_never_falls_back_to_classic() {
        let launch = LaunchConfig {
            tta: Some("dev".into()),
            ..Default::default()
        };
        assert_eq!(select_backend(&launch, false).ok(), cfg!(feature = "tta-lib").then_some(SessionBackend::TtaLib));
        for backend in [SessionBackend::ClassicTmuxCli, SessionBackend::ClassicTmuxTmax] {
            assert!(select_backend(
                &LaunchConfig {
                    session_backend: backend,
                    ..launch.clone()
                },
                true
            )
            .is_err());
        }
    }

    #[test]
    fn explicit_scaffolding_cannot_silently_degrade_to_classic_creation() {
        let mut launch = target();
        launch.scaffold = Some("my-generator".into());
        assert_eq!(select_backend(&launch, false).ok(), cfg!(feature = "tta-lib").then_some(SessionBackend::TtaLib));
        launch.session_backend = SessionBackend::ClassicTmuxCli;
        assert!(select_backend(&launch, true).is_err());
        launch.scaffold = Some("false".into());
        assert_eq!(select_backend(&launch, false).is_ok(), cfg!(feature = "classic-tmux-cli"));
    }

    #[test]
    fn create_only_and_attach_only_keep_classic_semantics() {
        for mut launch in [
            LaunchConfig {
                tmux_create: Some("dev".into()),
                ..Default::default()
            },
            LaunchConfig {
                tmux_attach: Some("dev".into()),
                ..Default::default()
            },
        ] {
            assert!(!matches!(
                select_backend(&launch, true).ok(),
                Some(SessionBackend::TtaLib | SessionBackend::TtaCli)
            ));
            launch.session_backend = SessionBackend::TtaCli;
            assert!(select_backend(&launch, true).is_err());
        }
    }

    #[test]
    fn tta_argv_protects_fragment_and_startup_words() {
        let launch = LaunchConfig {
            tmux: Some("-project".into()),
            ..Default::default()
        };
        let args = tta_argv(&launch, &["printf".into(), "a ; $b".into()]);
        let start = args.iter().position(|s| s == "--").unwrap();
        assert_eq!(&args[start..], &["--", "-project", "printf", "a ; $b"]);
    }

    #[test]
    #[serial_test::serial]
    fn project_roots_replace_the_whole_list_layer_by_layer() {
        use std::ffi::OsString;
        let saved: Vec<(String, Option<OsString>)> = ["TTA_ROOTS", "TTA_ROOT", "PROJECTS_DIR", "HOME"]
            .iter()
            .map(|key| (key.to_string(), std::env::var_os(key)))
            .collect();
        let restore = || {
            for (key, value) in &saved {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        };
        let cwd = std::env::current_dir().unwrap();
        let launch = |root: Option<&str>| LaunchConfig {
            session_root: root.map(str::to_owned),
            ..target()
        };
        std::env::set_var("TTA_ROOTS", r#"["ignored","/also-ignored"]"#);
        std::env::set_var("TTA_ROOT", "/ignored-root");
        std::env::set_var("PROJECTS_DIR", "/ignored-projects");
        std::env::set_var("HOME", "/home/base");
        // The explicit session root replaces every environment layer.
        assert_eq!(project_roots(&launch(Some("explicit"))).unwrap(), vec![cwd.join("explicit")]);
        // TTA_ROOTS replaces TTA_ROOT and PROJECTS_DIR; relative entries freeze
        // against the invocation cwd.
        std::env::set_var("TTA_ROOTS", r#"["personal","/work/team"]"#);
        assert_eq!(project_roots(&launch(None)).unwrap(), vec![cwd.join("personal"), PathBuf::from("/work/team")]);
        // An invalid active layer fails instead of falling through.
        for invalid in ["[]", r#"[""]"#, "not JSON"] {
            std::env::set_var("TTA_ROOTS", invalid);
            assert!(project_roots(&launch(None)).is_err(), "{invalid}");
        }
        // An empty value is unset; TTA_ROOT replaces PROJECTS_DIR.
        std::env::set_var("TTA_ROOTS", "");
        assert_eq!(project_roots(&launch(None)).unwrap(), vec![PathBuf::from("/ignored-root")]);
        // The HOME-relative PROJECTS_DIR layer remains the default.
        std::env::remove_var("TTA_ROOT");
        std::env::remove_var("PROJECTS_DIR");
        assert_eq!(project_roots(&launch(None)).unwrap(), vec![PathBuf::from("/home/base/src")]);
        restore();
    }

    #[cfg(feature = "classic-tmux-cli")]
    #[test]
    fn classic_argv_separates_formats_command_delimiters_and_startup_argv() {
        let launch = LaunchConfig {
            tmux: Some("a#b;".into()),
            tmux_socket: Some("/socket;".into()),
            ..Default::default()
        };
        let primary = classic_argv(&launch, &["/a b;".into()], None).unwrap();
        assert_eq!(primary[2], "/socket;"); // process option, not tmux command syntax
        assert!(primary.iter().any(|s| s == "a#{a:35}b\\;"));
        assert_eq!(primary.last().unwrap(), "exec '/a b;'");
        let group = classic_argv(&launch, &["ignored".into()], Some("$2")).unwrap();
        assert!(group.iter().any(|s| s == ";"));
        assert!(!group.iter().any(|s| s == "ignored"));
    }

    #[cfg(feature = "classic-tmux-cli")]
    #[test]
    fn remote_classic_probe_and_group_preserve_shell_and_tmux_metacharacters() {
        use std::os::unix::fs::PermissionsExt;
        fn scratch_dir(prefix: &str) -> tempfile::TempDir {
            if let Some(path) = std::env::var_os("TTA_TEST_TMPDIR").filter(|value| !value.is_empty()) {
                return tempfile::Builder::new().prefix(prefix).tempdir_in(path).unwrap();
            }
            match tempfile::Builder::new().prefix(prefix).tempdir_in("/tmp/opencode") {
                Ok(dir) => dir,
                Err(_) => tempfile::Builder::new().prefix(prefix).tempdir_in(std::env::temp_dir()).unwrap(),
            }
        }
        let dir = scratch_dir("tp-remote-argv-");
        let client = dir.path().join("fake tmux");
        let probe = dir.path().join("probe");
        let group = dir.path().join("group");
        std::fs::write(&client, "#!/bin/sh\ncase \"$1\" in\nhas-session) printf '%s\\0' \"$@\" > \"$PROBE\";;\nnew-session) printf '%s\\0' \"$@\" > \"$GROUP\";;\n*) exit 91;;\nesac\n").unwrap();
        std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o700)).unwrap();
        let launch = LaunchConfig {
            tmux: Some("a'$(false);".into()),
            tmux_program: client.to_str().unwrap().into(),
            ..Default::default()
        };
        let script = classic_remote_script(&launch, &["ignored startup".into()]).unwrap();
        let status = Command::new("sh")
            .args(["-c", &script])
            .env("PROBE", &probe)
            .env("GROUP", &group)
            .status()
            .unwrap();
        assert!(status.success());
        let words = |path| {
            String::from_utf8(std::fs::read(path).unwrap())
                .unwrap()
                .split_terminator('\0')
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert_eq!(words(probe), ["has-session", "-t", "=a'$(false)\\;"]);
        assert_eq!(
            words(group),
            ["new-session", "-t", "=a'$(false)\\;", ";", "set-option", "destroy-unattached", "keep-last"]
        );
    }
}
