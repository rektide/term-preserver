// pattern: Functional Core

use std::path::Path;

use crate::launcher::SessionBackend;
use crate::session::CommandSpec;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellFamily {
    Posix,
    Fish,
    Nushell,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellResolution {
    pub name: String,
    pub family: ShellFamily,
    pub is_fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxMode {
    CreateOrAttach(String),
    Create(String),
    Attach(String),
}

impl TmuxMode {
    pub fn session(&self) -> &str {
        match self {
            TmuxMode::CreateOrAttach(s) | TmuxMode::Create(s) | TmuxMode::Attach(s) => s,
        }
    }
}

/// The transport wrapping a resolved launch, if any. Local launches have no
/// transport; `--ssh`/`--mosh` carry the host. The sampler can only introspect
/// a *local* tmux server — a remote tmux lives behind ssh/mosh where the local
/// `tmux` binary cannot reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportKind {
    Local,
    Ssh(String),
    Mosh(String),
}

/// The minimal, movable summary of a resolved launch that the execution layer
/// (and the tmux sampler) needs. This is the "good state" derived from
/// [`LaunchConfig`] at config time and threaded through `RetryLoop →
/// ChildSession → TmuxSampler`, so the session/tmux layers never depend on the
/// full config shape.
///
/// Derived via [`LaunchConfig::context`] rather than returned from `resolve()`,
/// keeping argv resolution and introspection context as separate concerns (and
/// leaving `resolve()`'s signature — and its tests — untouched).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchContext {
    pub tmux_target: Option<TmuxMode>,
    pub transport: TransportKind,
    pub tmux_program: String,
    pub tmux_socket: Option<String>,
    pub native_queries: bool,
    pub exact_target: bool,
}

impl LaunchContext {
    /// The tmux target when the server is local and therefore queryable. Remote
    /// transports hide the tmux server behind ssh/mosh, so `None` there.
    pub fn local_tmux_target(&self) -> Option<&TmuxMode> {
        match (&self.transport, &self.tmux_target) {
            (TransportKind::Local, Some(mode)) => Some(mode),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchConfig {
    #[serde(default)]
    pub tta: Option<String>,
    #[serde(default)]
    pub session_backend: SessionBackend,
    #[serde(default)]
    pub session_root: Option<String>,
    #[serde(default)]
    pub tmux_socket: Option<String>,
    #[serde(default = "default_tmux_program")]
    pub tmux_program: String,
    #[serde(default = "default_tta_program")]
    pub tta_program: String,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub ssh: Option<String>,
    #[serde(default)]
    pub mosh: Option<String>,
    #[serde(default)]
    pub tmux: Option<String>,
    #[serde(default)]
    pub tmux_create: Option<String>,
    #[serde(default)]
    pub tmux_attach: Option<String>,
    #[serde(default)]
    pub dir: Option<String>,
    /// When true (default), `--tmux` create-or-attach joins the session's
    /// group when the primary already exists (independent current window,
    /// shared windows, satellite self-destroys on detach). `--no-tmux-group`
    /// restores plain attach-to-existing semantics. Requires tmux ≥ 3.4;
    /// shells without a verifiable conditional degrade to plain.
    #[serde(default = "default_launch_tmux_group")]
    pub tmux_group: bool,
    #[serde(default = "default_launch_login")]
    pub login: bool,
    #[serde(default = "default_launch_interactive")]
    pub interactive: bool,
}

impl Default for LaunchConfig {
    fn default() -> Self {
        Self {
            tta: None,
            session_backend: SessionBackend::Auto,
            session_root: None,
            tmux_socket: None,
            tmux_program: default_tmux_program(),
            tta_program: default_tta_program(),
            shell: None,
            ssh: None,
            mosh: None,
            tmux: None,
            tmux_create: None,
            tmux_attach: None,
            dir: None,
            tmux_group: default_launch_tmux_group(),
            login: default_launch_login(),
            interactive: default_launch_interactive(),
        }
    }
}

fn default_tmux_program() -> String {
    "tmux".into()
}
fn default_tta_program() -> String {
    "tta".into()
}

fn default_launch_tmux_group() -> bool {
    true
}

fn default_launch_login() -> bool {
    true
}

fn default_launch_interactive() -> bool {
    true
}

fn shell_basename(path: &str) -> String {
    Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path).to_string()
}

fn detect_shell_family(name: &str) -> ShellFamily {
    let base = shell_basename(name);
    match base.as_str() {
        "nu" | "nushell" => ShellFamily::Nushell,
        "fish" => ShellFamily::Fish,
        _ => ShellFamily::Posix,
    }
}

fn build_flags_string(login: bool, interactive: bool) -> String {
    match (login, interactive) {
        (true, true) => "-li".to_string(),
        (true, false) => "-l".to_string(),
        (false, true) => "-i".to_string(),
        (false, false) => String::new(),
    }
}

fn build_fallback_snippet(login: bool, interactive: bool, inner: Option<&InnerJob>, dir: Option<&str>) -> Vec<String> {
    let flags = build_flags_string(login, interactive);
    let cd_prefix = match dir {
        Some(d) => format!("cd {d} && "),
        None => String::new(),
    };

    // Grouped jobs degrade to `primary` here: the snippet re-execs through
    // `$SHELL -c` sight-unseen, and the conditional's syntax is family-locked.
    // Plain create-or-attach is family-neutral and keeps the fallback honest.
    let inner = inner.map(|job| job.plain());
    let snippet = match inner {
        None => format!(
            "{cd_prefix}if [ -n \"$SHELL\" ]; then exec \"$SHELL\" {flags}; \
             elif command -v zsh >/dev/null 2>&1; then exec zsh {flags}; \
             else exec sh {flags}; fi"
        ),
        Some(cmd) => format!(
            "{cd_prefix}if [ -n \"$SHELL\" ]; then exec \"$SHELL\" {flags} -c \"exec {cmd}\"; \
             elif command -v zsh >/dev/null 2>&1; then exec zsh {flags} -c \"exec {cmd}\"; \
             else exec sh {flags} -c \"exec {cmd}\"; fi"
        ),
    };

    vec!["sh".into(), "-c".into(), snippet]
}

fn resolve_shell(launch: &LaunchConfig, env_shell: Option<&str>) -> ShellResolution {
    if let Some(ref shell) = launch.shell {
        return ShellResolution {
            name: shell.clone(),
            family: detect_shell_family(shell),
            is_fallback: false,
        };
    }

    if let Some(shell) = env_shell {
        if !shell.is_empty() {
            return ShellResolution {
                name: shell.to_string(),
                family: detect_shell_family(shell),
                is_fallback: false,
            };
        }
    }

    ShellResolution {
        name: "sh".into(),
        family: ShellFamily::Posix,
        is_fallback: true,
    }
}

fn build_tmux_command(mode: &TmuxMode, dir: Option<&str>, trailing: &[String]) -> String {
    let session = mode.session();
    let dir_flag = dir.map(|d| format!(" -c {d}")).unwrap_or_default();
    let trailing_str = if trailing.is_empty() {
        String::new()
    } else {
        format!(" {}", trailing.join(" "))
    };

    match mode {
        TmuxMode::CreateOrAttach(_) => {
            format!("tmux new{dir_flag} -As {session}{trailing_str}")
        }
        TmuxMode::Create(_) => {
            format!("tmux new{dir_flag} -s {session}{trailing_str}")
        }
        TmuxMode::Attach(_) => {
            format!("tmux attach-session{dir_flag} -t {session}")
        }
    }
}

/// The satellite variant for grouped create-or-attach: joins the session's
/// group with an independent current window over the same shared windows.
/// `destroy-unattached keep-last` (tmux ≥ 3.4) makes this satellite destroy
/// itself when its client detaches while siblings — including the suffix-less
/// primary — remain; the last session of a group is always kept, so the
/// primary never needs the option itself.
///
/// No trailing command: the windows are shared with the group, whose initial
/// window already carries whatever the primary was given.
fn build_satellite_command(mode: &TmuxMode, dir: Option<&str>) -> String {
    let session = mode.session();
    let dir_flag = dir.map(|d| format!(" -c {d}")).unwrap_or_default();
    format!("tmux new{dir_flag} -t {session} \\; set destroy-unattached keep-last")
}

/// POSIX-sh conditional picking satellite vs primary at *run* time, so retries
/// and manual re-runs of the same resolved command re-probe `has-session`
/// (also on the remote host under ssh/mosh). `-t '=<name>'` demands an exact
/// session-name match so a lingering `dev-3` satellite never satisfies the
/// probe for `dev`. The single quotes are load-bearing for zsh: a bare `=dev`
/// word triggers zsh's `=command` expansion and dies with "dev not found",
/// while the quotes are inert to bash/fish and are stripped before tmux sees
/// `=name`. `2>/dev/null` hides has-session's "no such session" (and
/// no-server) noise; a missing server simply takes the primary branch.
fn posix_grouped_script(session: &str, satellite: &str, primary: &str) -> String {
    format!(
        "if tmux has-session -t '={session}' 2>/dev/null; \
         then exec {satellite}; else exec {primary}; fi"
    )
}

/// fish equivalent: `if`/`else`/`end` with `;` statement separators, which
/// fish shares with POSIX; `\;` escapes pass tmux its command separator.
/// The `'=<name>'` quoting matches [`posix_grouped_script`].
fn fish_grouped_script(session: &str, satellite: &str, primary: &str) -> String {
    format!(
        "if tmux has-session -t '={session}' 2>/dev/null; \
         exec {satellite}; else; exec {primary}; end"
    )
}

/// The innermost job for a launch. `Plain` is a single command string as
/// before; `Grouped` carries the create-or-attach pair whose choice is made
/// per execution by the shell ([`posix_grouped_script`] / [`fish_grouped_script`]).
///
/// Shells without a verifiable conditional form (nushell) and the
/// `$SHELL`-probing fallback snippet degrade to `primary` — plain
/// create-or-attach — rather than ship untested syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
enum InnerJob {
    Plain(String),
    Grouped { primary: String, satellite: String, session: String },
}

impl InnerJob {
    fn plain(&self) -> &str {
        match self {
            InnerJob::Plain(cmd) => cmd,
            InnerJob::Grouped { primary, .. } => primary,
        }
    }
}

fn resolve_inner_command(launch: &LaunchConfig, trailing: &[String]) -> Option<InnerJob> {
    let tmux_mode = launch.tmux_mode();

    if let Some(mode) = &tmux_mode {
        let primary = build_tmux_command(mode, launch.dir.as_deref(), trailing);
        let job = match mode {
            TmuxMode::CreateOrAttach(_) if launch.tmux_group => InnerJob::Grouped {
                primary,
                satellite: build_satellite_command(mode, launch.dir.as_deref()),
                session: mode.session().to_string(),
            },
            _ => InnerJob::Plain(primary),
        };
        return Some(job);
    }

    if !trailing.is_empty() {
        return Some(InnerJob::Plain(trailing.join(" ")));
    }

    None
}

fn build_shell_invocation(shell: &ShellResolution, inner: Option<&InnerJob>, dir: Option<&str>, login: bool, interactive: bool) -> Vec<String> {
    if shell.is_fallback {
        return build_fallback_snippet(login, interactive, inner, dir);
    }

    let flags = build_flags_string(login, interactive);

    /// Inner script for POSIX/fish: plain jobs get the usual `exec` prefix;
    /// grouped jobs become the family's run-time conditional.
    fn inner_script(job: &InnerJob, family: ShellFamily) -> String {
        match job {
            InnerJob::Plain(cmd) => format!("exec {cmd}"),
            InnerJob::Grouped { primary, satellite, session } => match family {
                ShellFamily::Posix => posix_grouped_script(session, satellite, primary),
                ShellFamily::Fish => fish_grouped_script(session, satellite, primary),
                ShellFamily::Nushell => format!("exec {}", job.plain()),
            },
        }
    }

    match shell.family {
        ShellFamily::Nushell => {
            let mut cmd = vec![shell.name.clone()];
            match (inner, dir) {
                (None, None) => {}
                (Some(job), None) => {
                    cmd.push("-c".into());
                    // No `exec` prefix for nushell (matches the historic
                    // form), and grouped jobs degrade to the plain primary:
                    // the conditional's nu syntax is unverified (no local
                    // binary to test against).
                    cmd.push(job.plain().to_string());
                }
                (None, Some(d)) => {
                    cmd.push("-c".into());
                    cmd.push(format!("cd {d}; nu"));
                }
                (Some(job), Some(d)) => {
                    cmd.push("-c".into());
                    cmd.push(format!("cd {d}; {}", job.plain()));
                }
            }
            cmd
        }
        ShellFamily::Posix | ShellFamily::Fish => {
            let mut cmd = vec![shell.name.clone()];

            let c_arg = match (inner, dir) {
                (None, None) => None,
                (Some(job), None) => Some(inner_script(job, shell.family)),
                (None, Some(d)) => Some(format!("cd {d} && exec {name} {flags}", name = shell.name, flags = flags)),
                (Some(job), Some(d)) => Some(format!("cd {d} && {}", inner_script(job, shell.family))),
            };

            // The `(None, Some(dir))` arm re-execs the same shell with `{flags}`
            // *inside* the -c string, so the outer shell carrying them too would
            // source login/rc files twice. Suppress the outer flags there; every
            // other arm needs them so the pre-exec environment is set up once.
            let suppress_outer_flags = matches!((inner, dir), (None, Some(_)));
            let outer_flags = if suppress_outer_flags { String::new() } else { flags };

            match c_arg {
                Some(c_arg) => {
                    if !outer_flags.is_empty() {
                        cmd.push(outer_flags);
                    }
                    cmd.push("-c".into());
                    cmd.push(c_arg);
                }
                None => {
                    if !outer_flags.is_empty() {
                        cmd.push(outer_flags);
                    }
                }
            }
            cmd
        }
    }
}

/// Single-quote a token for a POSIX shell, leaving already-safe tokens bare.
/// Embedded single quotes become the standard `'\''` escape.
pub(crate) fn shell_quote(arg: &str) -> String {
    let is_safe = !arg.is_empty() && arg.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=@,+".contains(c));
    if is_safe {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

/// Join an argv into one shell-quoted command line that survives being
/// re-split by a remote shell (ssh concatenates and re-parses the command).
pub(crate) fn shell_join(args: &[String]) -> String {
    args.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ")
}

pub(crate) fn wrap_transport(cmd: Vec<String>, ssh: Option<&str>, mosh: Option<&str>) -> Vec<String> {
    if let Some(host) = ssh {
        // `-t` forces remote PTY allocation: ssh disables the remote tty whenever
        // a command is given (we always give one), and without it an interactive
        // remote shell is degraded and tmux fails with "open terminal failed".
        //
        // ssh concatenates the remote command argv into a single string and runs
        // it through the remote login shell, so we hand it ONE shell-quoted line
        // rather than separate tokens (which would lose argument grouping). ssh
        // also has no `--` option terminator -- a `--` ends up in the remote
        // command and breaks it -- so it is omitted here. mosh differs on both
        // counts (it passes argv through and uses `--`), so its branch is left
        // alone.
        vec!["ssh".into(), "-t".into(), host.into(), shell_join(&cmd)]
    } else if let Some(host) = mosh {
        let mut wrapped = vec!["mosh".into(), host.into(), "--".into()];
        wrapped.extend(cmd);
        wrapped
    } else {
        cmd
    }
}

impl LaunchConfig {
    pub fn tmux_mode(&self) -> Option<TmuxMode> {
        if let Some(ref s) = self.tta {
            return Some(TmuxMode::CreateOrAttach(s.clone()));
        }
        if let Some(ref s) = self.tmux {
            return Some(TmuxMode::CreateOrAttach(s.clone()));
        }
        if let Some(ref s) = self.tmux_create {
            return Some(TmuxMode::Create(s.clone()));
        }
        if let Some(ref s) = self.tmux_attach {
            return Some(TmuxMode::Attach(s.clone()));
        }
        None
    }

    /// A launch is "active" (and therefore worth resolving instead of taking the
    /// raw `-- command` path) whenever any field differs from the defaults.
    /// Comparing against `Default` stays correct as fields are added, rather than
    /// an enumeration that silently drifts.
    pub fn is_active(&self) -> bool {
        *self != Self::default()
    }

    /// Derive the movable [`LaunchContext`] the execution layer needs: the tmux
    /// target (if any) and the transport. Pure — read it alongside `resolve()`
    /// in the imperative shell, then thread it through to the sampler.
    pub fn context(&self) -> LaunchContext {
        let transport = if let Some(host) = &self.ssh {
            TransportKind::Ssh(host.clone())
        } else if let Some(host) = &self.mosh {
            TransportKind::Mosh(host.clone())
        } else {
            TransportKind::Local
        };
        LaunchContext {
            tmux_target: self.tmux_mode(),
            transport,
            tmux_program: self.tmux_program.clone(),
            tmux_socket: self.tmux_socket.clone(),
            native_queries: matches!(self.session_backend, SessionBackend::TtaLib | SessionBackend::ClassicTmuxTmax)
                || (self.session_backend == SessionBackend::Auto && cfg!(any(feature = "tta-lib", feature = "classic-tmux-tmax"))),
            exact_target: self.tta.is_none()
                && (self.tmux_create.is_some()
                    || self.tmux_attach.is_some()
                    || matches!(self.session_backend, SessionBackend::ClassicTmuxCli | SessionBackend::ClassicTmuxTmax)),
        }
    }

    pub(crate) fn validate(&self) -> eyre::Result<()> {
        if self.ssh.is_some() && self.mosh.is_some() {
            eyre::bail!("--ssh and --mosh are mutually exclusive");
        }

        let tmux_count = [&self.tta, &self.tmux, &self.tmux_create, &self.tmux_attach]
            .iter()
            .filter(|o| o.is_some())
            .count();
        if tmux_count > 1 {
            eyre::bail!("--tta, --tmux, --tmux-create, and --tmux-attach are mutually exclusive");
        }

        Ok(())
    }

    /// `env_shell` is the caller's `$SHELL` (read in the imperative shell), used
    /// only when neither `--shell` nor a config shell is set. Passing it in keeps
    /// this resolution pipeline pure and deterministic in tests.
    pub fn resolve(&self, trailing: &[String], env_shell: Option<&str>) -> eyre::Result<CommandSpec> {
        self.validate()?;
        if self.tmux_mode().is_some() && !cfg!(feature = "classic-tmux-cli") {
            eyre::bail!("classic tmux CLI launching was not compiled; use launcher::prepare for backend selection");
        }

        let shell = resolve_shell(self, env_shell);
        let inner = resolve_inner_command(self, trailing);
        let dir_for_shell = if self.tmux_mode().is_some() { None } else { self.dir.as_deref() };
        let shell_cmd = build_shell_invocation(&shell, inner.as_ref(), dir_for_shell, self.login, self.interactive);
        let full_cmd = wrap_transport(shell_cmd, self.ssh.as_deref(), self.mosh.as_deref());

        CommandSpec::try_from(full_cmd)
    }
}

#[cfg(all(test, feature = "classic-tmux-cli"))]
mod tests {
    use super::*;

    fn launch() -> LaunchConfig {
        LaunchConfig {
            shell: None,
            ssh: None,
            mosh: None,
            tmux: None,
            tmux_create: None,
            tmux_attach: None,
            dir: None,
            tmux_group: true,
            login: true,
            interactive: true,
            ..LaunchConfig::default()
        }
    }

    fn launch_with_shell(shell: &str) -> LaunchConfig {
        let mut l = launch();
        l.shell = Some(shell.into());
        l
    }

    // --- shell family detection ---

    #[test]
    fn detect_posix_shells() {
        assert_eq!(detect_shell_family("zsh"), ShellFamily::Posix);
        assert_eq!(detect_shell_family("bash"), ShellFamily::Posix);
        assert_eq!(detect_shell_family("sh"), ShellFamily::Posix);
        assert_eq!(detect_shell_family("dash"), ShellFamily::Posix);
    }

    #[test]
    fn detect_fish() {
        assert_eq!(detect_shell_family("fish"), ShellFamily::Fish);
    }

    #[test]
    fn detect_nushell() {
        assert_eq!(detect_shell_family("nu"), ShellFamily::Nushell);
        assert_eq!(detect_shell_family("nushell"), ShellFamily::Nushell);
    }

    #[test]
    fn detect_unknown_defaults_to_posix() {
        assert_eq!(detect_shell_family("unknown-shell"), ShellFamily::Posix);
    }

    #[test]
    fn detect_handles_full_paths() {
        assert_eq!(detect_shell_family("/bin/zsh"), ShellFamily::Posix);
        assert_eq!(detect_shell_family("/usr/bin/fish"), ShellFamily::Fish);
        assert_eq!(detect_shell_family("/home/user/nu"), ShellFamily::Nushell);
    }

    #[test]
    fn shell_basename_strips_path() {
        assert_eq!(shell_basename("/bin/zsh"), "zsh");
        assert_eq!(shell_basename("/usr/local/bin/fish"), "fish");
        assert_eq!(shell_basename("zsh"), "zsh");
    }

    // --- flags ---

    #[test]
    fn flags_login_interactive() {
        assert_eq!(build_flags_string(true, true), "-li");
    }

    #[test]
    fn flags_login_only() {
        assert_eq!(build_flags_string(true, false), "-l");
    }

    #[test]
    fn flags_interactive_only() {
        assert_eq!(build_flags_string(false, true), "-i");
    }

    #[test]
    fn flags_neither() {
        assert_eq!(build_flags_string(false, false), "");
    }

    // --- shell invocation: POSIX ---

    #[test]
    fn posix_bare_shell() {
        let shell = ShellResolution {
            name: "zsh".into(),
            family: ShellFamily::Posix,
            is_fallback: false,
        };
        let cmd = build_shell_invocation(&shell, None, None, true, true);
        assert_eq!(cmd, vec!["zsh", "-li"]);
    }

    #[test]
    fn posix_with_inner() {
        let shell = ShellResolution {
            name: "zsh".into(),
            family: ShellFamily::Posix,
            is_fallback: false,
        };
        let cmd = build_shell_invocation(&shell, Some(&InnerJob::Plain("tmux new -As dev".into())), None, true, true);
        assert_eq!(cmd, vec!["zsh", "-li", "-c", "exec tmux new -As dev"]);
    }

    #[test]
    fn posix_no_login_no_interactive() {
        let shell = ShellResolution {
            name: "bash".into(),
            family: ShellFamily::Posix,
            is_fallback: false,
        };
        let cmd = build_shell_invocation(&shell, None, None, false, false);
        assert_eq!(cmd, vec!["bash"]);
    }

    #[test]
    fn posix_login_only_with_inner() {
        let shell = ShellResolution {
            name: "zsh".into(),
            family: ShellFamily::Posix,
            is_fallback: false,
        };
        let cmd = build_shell_invocation(&shell, Some(&InnerJob::Plain("echo hi".into())), None, true, false);
        assert_eq!(cmd, vec!["zsh", "-l", "-c", "exec echo hi"]);
    }

    // --- shell invocation: nushell ---

    #[test]
    fn nushell_bare() {
        let shell = ShellResolution {
            name: "nu".into(),
            family: ShellFamily::Nushell,
            is_fallback: false,
        };
        let cmd = build_shell_invocation(&shell, None, None, true, true);
        assert_eq!(cmd, vec!["nu"]);
    }

    #[test]
    fn nushell_with_inner() {
        let shell = ShellResolution {
            name: "nu".into(),
            family: ShellFamily::Nushell,
            is_fallback: false,
        };
        let cmd = build_shell_invocation(&shell, Some(&InnerJob::Plain("tmux new -As dev".into())), None, true, true);
        assert_eq!(cmd, vec!["nu", "-c", "tmux new -As dev"]);
    }

    // --- shell invocation: fish ---

    #[test]
    fn fish_bare() {
        let shell = ShellResolution {
            name: "fish".into(),
            family: ShellFamily::Fish,
            is_fallback: false,
        };
        let cmd = build_shell_invocation(&shell, None, None, true, true);
        assert_eq!(cmd, vec!["fish", "-li"]);
    }

    #[test]
    fn fish_with_inner() {
        let shell = ShellResolution {
            name: "fish".into(),
            family: ShellFamily::Fish,
            is_fallback: false,
        };
        let cmd = build_shell_invocation(&shell, Some(&InnerJob::Plain("tmux new -As dev".into())), None, true, true);
        assert_eq!(cmd, vec!["fish", "-li", "-c", "exec tmux new -As dev"]);
    }

    // --- dir handling ---

    #[test]
    fn posix_dir_no_inner() {
        let shell = ShellResolution {
            name: "zsh".into(),
            family: ShellFamily::Posix,
            is_fallback: false,
        };
        let cmd = build_shell_invocation(&shell, None, Some("~/src/foo"), true, true);
        assert_eq!(cmd, vec!["zsh", "-c", "cd ~/src/foo && exec zsh -li"]);
    }

    #[test]
    fn posix_dir_with_inner() {
        let shell = ShellResolution {
            name: "zsh".into(),
            family: ShellFamily::Posix,
            is_fallback: false,
        };
        let cmd = build_shell_invocation(&shell, Some(&InnerJob::Plain("make test".into())), Some("~/src/foo"), true, true);
        assert_eq!(cmd, vec!["zsh", "-li", "-c", "cd ~/src/foo && exec make test"]);
    }

    #[test]
    fn nushell_dir_no_inner() {
        let shell = ShellResolution {
            name: "nu".into(),
            family: ShellFamily::Nushell,
            is_fallback: false,
        };
        let cmd = build_shell_invocation(&shell, None, Some("~/src/foo"), true, true);
        assert_eq!(cmd, vec!["nu", "-c", "cd ~/src/foo; nu"]);
    }

    // --- fallback snippet ---

    #[test]
    fn fallback_bare_shell() {
        let cmd = build_fallback_snippet(true, true, None, None);
        assert_eq!(cmd[0], "sh");
        assert_eq!(cmd[1], "-c");
        assert!(cmd[2].contains("$SHELL"));
        assert!(cmd[2].contains("zsh"));
        assert!(cmd[2].contains("exec sh -li"));
    }

    #[test]
    fn fallback_with_inner() {
        let cmd = build_fallback_snippet(true, true, Some(&InnerJob::Plain("tmux new -As dev".into())), None);
        assert!(cmd[2].contains("exec tmux new -As dev"));
    }

    #[test]
    fn fallback_with_dir() {
        let cmd = build_fallback_snippet(true, true, None, Some("~/foo"));
        assert!(cmd[2].starts_with("cd ~/foo && "));
    }

    // --- tmux command construction ---

    #[test]
    fn tmux_create_or_attach() {
        let mode = TmuxMode::CreateOrAttach("dev".into());
        assert_eq!(build_tmux_command(&mode, None, &[]), "tmux new -As dev");
    }

    #[test]
    fn tmux_create() {
        let mode = TmuxMode::Create("dev".into());
        assert_eq!(build_tmux_command(&mode, None, &[]), "tmux new -s dev");
    }

    #[test]
    fn tmux_attach() {
        let mode = TmuxMode::Attach("dev".into());
        assert_eq!(build_tmux_command(&mode, None, &[]), "tmux attach-session -t dev");
    }

    #[test]
    fn tmux_with_dir() {
        let mode = TmuxMode::CreateOrAttach("dev".into());
        assert_eq!(build_tmux_command(&mode, Some("~/src/foo"), &[]), "tmux new -c ~/src/foo -As dev");
    }

    #[test]
    fn tmux_with_trailing() {
        let mode = TmuxMode::CreateOrAttach("dev".into());
        assert_eq!(build_tmux_command(&mode, None, &["vim".into()]), "tmux new -As dev vim");
    }

    #[test]
    fn tmux_attach_with_dir() {
        let mode = TmuxMode::Attach("dev".into());
        assert_eq!(build_tmux_command(&mode, Some("~/foo"), &[]), "tmux attach-session -c ~/foo -t dev");
    }

    // --- grouped satellites ---

    #[test]
    fn satellite_command() {
        let mode = TmuxMode::CreateOrAttach("dev".into());
        assert_eq!(build_satellite_command(&mode, None), r"tmux new -t dev \; set destroy-unattached keep-last");
    }

    #[test]
    fn satellite_command_with_dir() {
        let mode = TmuxMode::CreateOrAttach("dev".into());
        assert_eq!(
            build_satellite_command(&mode, Some("~/src/foo")),
            r"tmux new -c ~/src/foo -t dev \; set destroy-unattached keep-last"
        );
    }

    #[test]
    fn posix_grouped_script_exact_match_probe() {
        let script = posix_grouped_script("dev", r"tmux new -t dev \; set destroy-unattached keep-last", "tmux new -As dev");
        assert_eq!(
            script,
            "if tmux has-session -t '=dev' 2>/dev/null; \
             then exec tmux new -t dev \\; set destroy-unattached keep-last; \
             else exec tmux new -As dev; fi"
        );
    }

    #[test]
    fn fish_grouped_script_shape() {
        let script = fish_grouped_script("dev", "tmux new -t dev", "tmux new -As dev");
        assert_eq!(
            script,
            "if tmux has-session -t '=dev' 2>/dev/null; \
             exec tmux new -t dev; else; exec tmux new -As dev; end"
        );
    }

    #[test]
    fn posix_grouped_shell_invocation() {
        let shell = ShellResolution {
            name: "zsh".into(),
            family: ShellFamily::Posix,
            is_fallback: false,
        };
        let job = InnerJob::Grouped {
            primary: "tmux new -As dev".into(),
            satellite: r"tmux new -t dev \; set destroy-unattached keep-last".into(),
            session: "dev".into(),
        };
        let cmd = build_shell_invocation(&shell, Some(&job), None, true, true);
        assert_eq!(cmd[0], "zsh");
        assert_eq!(cmd[1], "-li");
        assert_eq!(cmd[2], "-c");
        assert_eq!(
            cmd[3],
            "if tmux has-session -t '=dev' 2>/dev/null; \
             then exec tmux new -t dev \\; set destroy-unattached keep-last; \
             else exec tmux new -As dev; fi"
        );
    }

    #[test]
    fn fish_grouped_shell_invocation() {
        let shell = ShellResolution {
            name: "fish".into(),
            family: ShellFamily::Fish,
            is_fallback: false,
        };
        let job = InnerJob::Grouped {
            primary: "tmux new -As dev".into(),
            satellite: r"tmux new -t dev \; set destroy-unattached keep-last".into(),
            session: "dev".into(),
        };
        let cmd = build_shell_invocation(&shell, Some(&job), None, true, true);
        assert_eq!(cmd[0], "fish");
        assert_eq!(cmd[1], "-li");
        assert_eq!(cmd[2], "-c");
        assert_eq!(
            cmd[3],
            "if tmux has-session -t '=dev' 2>/dev/null; \
             exec tmux new -t dev \\; set destroy-unattached keep-last; \
             else; exec tmux new -As dev; end"
        );
    }

    #[test]
    fn nushell_grouped_degrades_to_plain_primary() {
        let shell = ShellResolution {
            name: "nu".into(),
            family: ShellFamily::Nushell,
            is_fallback: false,
        };
        let job = InnerJob::Grouped {
            primary: "tmux new -As dev".into(),
            satellite: r"tmux new -t dev \; set destroy-unattached keep-last".into(),
            session: "dev".into(),
        };
        let cmd = build_shell_invocation(&shell, Some(&job), None, true, true);
        assert_eq!(cmd, vec!["nu", "-c", "tmux new -As dev"]);
    }

    #[test]
    fn fallback_grouped_degrades_to_plain_primary() {
        let job = InnerJob::Grouped {
            primary: "tmux new -As dev".into(),
            satellite: r"tmux new -t dev \; set destroy-unattached keep-last".into(),
            session: "dev".into(),
        };
        let cmd = build_fallback_snippet(true, true, Some(&job), None);
        assert!(cmd[2].contains("exec tmux new -As dev"));
        assert!(!cmd[2].contains("has-session"));
    }

    // --- transport wrapping ---

    #[test]
    fn wrap_ssh() {
        let cmd = wrap_transport(vec!["zsh".into(), "-li".into()], Some("w"), None);
        assert_eq!(cmd, vec!["ssh", "-t", "w", "zsh -li"]);
    }

    #[test]
    fn shell_quote_leaves_safe_tokens_bare() {
        assert_eq!(shell_quote("zsh"), "zsh");
        assert_eq!(shell_quote("-li"), "-li");
        assert_eq!(shell_quote("/usr/bin/zsh"), "/usr/bin/zsh");
        assert_eq!(shell_quote("user@host"), "user@host");
    }

    #[test]
    fn shell_quote_wraps_spaces_and_specials() {
        assert_eq!(shell_quote("exec tmux new -As dev"), "'exec tmux new -As dev'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn shell_join_quotes_each_token() {
        let joined = shell_join(&["zsh".into(), "-li".into(), "-c".into(), "exec tmux new -As dev".into()]);
        assert_eq!(joined, "zsh -li -c 'exec tmux new -As dev'");
    }

    #[test]
    fn wrap_mosh() {
        let cmd = wrap_transport(vec!["zsh".into(), "-li".into()], None, Some("w"));
        assert_eq!(cmd, vec!["mosh", "w", "--", "zsh", "-li"]);
    }

    #[test]
    fn wrap_neither() {
        let cmd = wrap_transport(vec!["zsh".into()], None, None);
        assert_eq!(cmd, vec!["zsh"]);
    }

    // --- validation ---

    #[test]
    fn validate_rejects_ssh_and_mosh() {
        let mut l = launch();
        l.ssh = Some("w".into());
        l.mosh = Some("w".into());
        assert!(l.validate().is_err());
    }

    #[test]
    fn validate_rejects_multiple_tmux_flags() {
        let mut l = launch();
        l.tmux = Some("dev".into());
        l.tmux_create = Some("dev".into());
        assert!(l.validate().is_err());
    }

    #[test]
    fn validate_accepts_single_transport_and_tmux() {
        let mut l = launch();
        l.ssh = Some("w".into());
        l.tmux = Some("dev".into());
        assert!(l.validate().is_ok());
    }

    // --- is_active ---

    #[test]
    fn is_active_false_for_empty() {
        let l = launch();
        assert!(!l.is_active());
    }

    #[test]
    fn is_active_true_when_shell_set() {
        let l = launch_with_shell("zsh");
        assert!(l.is_active());
    }

    #[test]
    fn is_active_true_when_no_login() {
        let mut l = launch();
        l.login = false;
        assert!(l.is_active());
    }

    #[test]
    fn is_active_true_when_no_tmux_group() {
        let mut l = launch();
        l.tmux_group = false;
        assert!(l.is_active());
    }

    // --- context() / LaunchContext ---

    #[test]
    fn context_local_has_no_transport() {
        let l = launch_with_shell("zsh");
        let ctx = l.context();
        assert_eq!(ctx.transport, TransportKind::Local);
        assert!(ctx.tmux_target.is_none());
        assert!(ctx.local_tmux_target().is_none());
    }

    #[test]
    fn context_local_tmux_is_queryable() {
        let mut l = launch_with_shell("zsh");
        l.tmux = Some("dev".into());
        let ctx = l.context();
        assert_eq!(ctx.transport, TransportKind::Local);
        assert_eq!(ctx.local_tmux_target(), Some(&TmuxMode::CreateOrAttach("dev".into())));
    }

    #[test]
    fn context_ssh_tmux_is_not_queryable() {
        // Remote transport hides the tmux server behind ssh: the local `tmux`
        // binary cannot reach it, so there is no local target to sample.
        let mut l = launch_with_shell("zsh");
        l.ssh = Some("w".into());
        l.tmux = Some("dev".into());
        let ctx = l.context();
        assert_eq!(ctx.transport, TransportKind::Ssh("w".into()));
        assert!(ctx.local_tmux_target().is_none());
    }

    #[test]
    fn context_mosh_transport() {
        let mut l = launch_with_shell("zsh");
        l.mosh = Some("w".into());
        assert_eq!(l.context().transport, TransportKind::Mosh("w".into()));
    }

    // --- full resolution pipeline ---

    #[test]
    fn resolve_local_tmux() {
        let mut l = launch_with_shell("zsh");
        l.tmux = Some("dev".into());
        let spec = l.resolve(&[], None).unwrap();
        assert_eq!(spec.program, "zsh");
        assert_eq!(
            spec.args,
            vec![
                "-li",
                "-c",
                "if tmux has-session -t '=dev' 2>/dev/null; \
                 then exec tmux new -t dev \\; set destroy-unattached keep-last; \
                 else exec tmux new -As dev; fi"
            ]
        );
    }

    #[test]
    fn resolve_local_tmux_no_group() {
        let mut l = launch_with_shell("zsh");
        l.tmux = Some("dev".into());
        l.tmux_group = false;
        let spec = l.resolve(&[], None).unwrap();
        assert_eq!(spec.program, "zsh");
        assert_eq!(spec.args, vec!["-li", "-c", "exec tmux new -As dev"]);
    }

    #[test]
    fn resolve_mosh_tmux() {
        let mut l = launch_with_shell("zsh");
        l.mosh = Some("w".into());
        l.tmux = Some("compfuzor".into());
        let spec = l.resolve(&[], None).unwrap();
        assert_eq!(spec.program, "mosh");
        assert_eq!(
            spec.args,
            vec![
                "w",
                "--",
                "zsh",
                "-li",
                "-c",
                "if tmux has-session -t '=compfuzor' 2>/dev/null; \
                 then exec tmux new -t compfuzor \\; set destroy-unattached keep-last; \
                 else exec tmux new -As compfuzor; fi"
            ]
        );
    }

    #[test]
    fn resolve_mosh_nu_tmux() {
        // Nushell degrades grouping to plain create-or-attach.
        let mut l = launch_with_shell("nu");
        l.mosh = Some("w".into());
        l.tmux = Some("compfuzor".into());
        let spec = l.resolve(&[], None).unwrap();
        assert_eq!(spec.program, "mosh");
        assert_eq!(spec.args, vec!["w", "--", "nu", "-c", "tmux new -As compfuzor"]);
    }

    #[test]
    fn resolve_tmux_create_stays_plain() {
        let mut l = launch_with_shell("zsh");
        l.tmux_create = Some("dev".into());
        let spec = l.resolve(&[], None).unwrap();
        assert_eq!(spec.args, vec!["-li", "-c", "exec tmux new -s dev"]);
    }

    #[test]
    fn resolve_tmux_attach_stays_plain() {
        let mut l = launch_with_shell("zsh");
        l.tmux_attach = Some("dev".into());
        let spec = l.resolve(&[], None).unwrap();
        assert_eq!(spec.args, vec!["-li", "-c", "exec tmux attach-session -t dev"]);
    }

    #[test]
    fn resolve_tmux_with_dir_and_trailing() {
        // `-c` lands in both branches; the trailing command belongs to the
        // fresh primary only — the satellite shares the group's windows.
        let mut l = launch_with_shell("zsh");
        l.tmux = Some("dev".into());
        l.dir = Some("~/src/foo".into());
        let spec = l.resolve(&["vim".into()], None).unwrap();
        assert_eq!(spec.program, "zsh");
        assert_eq!(
            spec.args,
            vec![
                "-li",
                "-c",
                "if tmux has-session -t '=dev' 2>/dev/null; \
                 then exec tmux new -c ~/src/foo -t dev \\; \
                 set destroy-unattached keep-last; \
                 else exec tmux new -c ~/src/foo -As dev vim; fi"
            ]
        );
    }

    #[test]
    fn resolve_dir_without_tmux() {
        let mut l = launch_with_shell("zsh");
        l.dir = Some("~/src/foo".into());
        let spec = l.resolve(&[], None).unwrap();
        assert_eq!(spec.program, "zsh");
        assert_eq!(spec.args, vec!["-c", "cd ~/src/foo && exec zsh -li"]);
    }

    #[test]
    fn resolve_dir_with_trailing_no_tmux() {
        let mut l = launch_with_shell("zsh");
        l.dir = Some("~/src/foo".into());
        let spec = l.resolve(&["make".into(), "test".into()], None).unwrap();
        assert_eq!(spec.program, "zsh");
        assert_eq!(spec.args, vec!["-li", "-c", "cd ~/src/foo && exec make test"]);
    }

    #[test]
    fn resolve_ssh_tmux_forces_remote_pty() {
        let mut l = launch_with_shell("zsh");
        l.ssh = Some("w".into());
        l.tmux = Some("dev".into());
        let spec = l.resolve(&[], None).unwrap();
        assert_eq!(spec.program, "ssh");
        assert_eq!(
            spec.args,
            vec![
                "-t",
                "w",
                // The -c script is shell_quote()d as one token for the ssh
                // remote line: embedded ' become '\'' — including the
                // '=<session>' exact-match quoting.
                "zsh -li -c 'if tmux has-session -t '\\''=dev'\\'' 2>/dev/null; \
                 then exec tmux new -t dev \\; set destroy-unattached keep-last; \
                 else exec tmux new -As dev; fi'"
            ]
        );
    }

    #[test]
    fn resolve_ssh_without_tmux() {
        let mut l = launch_with_shell("zsh");
        l.ssh = Some("w".into());
        let spec = l.resolve(&[], None).unwrap();
        assert_eq!(spec.program, "ssh");
        assert_eq!(spec.args, vec!["-t", "w", "zsh -li"]);
    }

    #[test]
    fn resolve_bare_shell_override() {
        let l = launch_with_shell("bash");
        let spec = l.resolve(&[], None).unwrap();
        assert_eq!(spec.program, "bash");
        assert_eq!(spec.args, vec!["-li"]);
    }

    #[test]
    fn resolve_no_login_no_interactive() {
        let mut l = launch_with_shell("zsh");
        l.login = false;
        l.interactive = false;
        let spec = l.resolve(&[], None).unwrap();
        assert_eq!(spec.program, "zsh");
        assert!(spec.args.is_empty());
    }

    #[test]
    fn resolve_errors_on_ssh_and_mosh() {
        let mut l = launch_with_shell("zsh");
        l.ssh = Some("w".into());
        l.mosh = Some("w".into());
        assert!(l.resolve(&[], None).is_err());
    }

    #[test]
    fn resolve_errors_on_multiple_tmux_flags() {
        let mut l = launch_with_shell("zsh");
        l.tmux = Some("dev".into());
        l.tmux_attach = Some("dev".into());
        assert!(l.resolve(&[], None).is_err());
    }
}
