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
/// keeping argv resolution and introspection context as separate concerns.
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
    pub scaffold: Option<String>,
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
    /// restores plain attach-to-existing semantics. The selected session
    /// backend applies grouping at execution time (tmux ≥ 3.4 semantics);
    /// shell-only launches ignore it.
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
            scaffold: None,
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

fn build_fallback_snippet(login: bool, interactive: bool, inner: Option<&str>, dir: Option<&str>) -> Vec<String> {
    let flags = build_flags_string(login, interactive);
    let cd_prefix = match dir {
        Some(d) => format!("cd {d} && "),
        None => String::new(),
    };

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

fn build_shell_invocation(shell: &ShellResolution, inner: Option<&str>, dir: Option<&str>, login: bool, interactive: bool) -> Vec<String> {
    if shell.is_fallback {
        return build_fallback_snippet(login, interactive, inner, dir);
    }

    let flags = build_flags_string(login, interactive);

    match shell.family {
        ShellFamily::Nushell => {
            let mut cmd = vec![shell.name.clone()];
            match (inner, dir) {
                (None, None) => {}
                (Some(inner), None) => {
                    cmd.push("-c".into());
                    // No `exec` prefix for nushell (matches the historic form).
                    cmd.push(inner.to_string());
                }
                (None, Some(d)) => {
                    cmd.push("-c".into());
                    cmd.push(format!("cd {d}; nu"));
                }
                (Some(inner), Some(d)) => {
                    cmd.push("-c".into());
                    cmd.push(format!("cd {d}; {inner}"));
                }
            }
            cmd
        }
        ShellFamily::Posix | ShellFamily::Fish => {
            let mut cmd = vec![shell.name.clone()];

            let c_arg = match (inner, dir) {
                (None, None) => None,
                (Some(inner), None) => Some(format!("exec {inner}")),
                (None, Some(d)) => Some(format!("cd {d} && exec {name} {flags}", name = shell.name, flags = flags)),
                (Some(inner), Some(d)) => Some(format!("cd {d} && exec {inner}")),
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

    /// Resolve this launch into a repeatable [`CommandSpec`]. A session target
    /// (`--tta`/`--tmux`/`--tmux-create`/`--tmux-attach`) routes through
    /// [`crate::launcher::prepare`] for backend selection and policy — there
    /// is no classic-renderer fallback; an unavailable backend is an error.
    /// `env_shell` is the caller's `$SHELL` (read in the imperative shell), used
    /// only when neither `--shell` nor a config shell is set. Shell-only
    /// resolution is pure; local session preparation also freezes the endpoint
    /// and worker executable without querying the server.
    pub fn resolve(&self, trailing: &[String], env_shell: Option<&str>) -> eyre::Result<CommandSpec> {
        crate::launcher::prepare(self, trailing, env_shell)
    }

    /// Shell-only resolution: wrap the trailing command (if any) in the
    /// selected shell family's invocation and the transport. No session target
    /// may be present — [`crate::launcher::prepare`] owns target routing.
    pub(crate) fn resolve_shell_command(&self, trailing: &[String], env_shell: Option<&str>) -> eyre::Result<CommandSpec> {
        self.validate()?;
        if self.tmux_mode().is_some() {
            eyre::bail!("a session target requires backend selection; use launcher::prepare");
        }

        let shell = resolve_shell(self, env_shell);
        let inner = (!trailing.is_empty()).then(|| trailing.join(" "));
        let shell_cmd = build_shell_invocation(&shell, inner.as_deref(), self.dir.as_deref(), self.login, self.interactive);
        let full_cmd = wrap_transport(shell_cmd, self.ssh.as_deref(), self.mosh.as_deref());

        CommandSpec::try_from(full_cmd)
    }
}

#[cfg(test)]
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
        let cmd = build_shell_invocation(&shell, Some("tmux new -As dev"), None, true, true);
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
        let cmd = build_shell_invocation(&shell, Some("echo hi"), None, true, false);
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
        let cmd = build_shell_invocation(&shell, Some("tmux new -As dev"), None, true, true);
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
        let cmd = build_shell_invocation(&shell, Some("tmux new -As dev"), None, true, true);
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
        let cmd = build_shell_invocation(&shell, Some("make test"), Some("~/src/foo"), true, true);
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
        let cmd = build_fallback_snippet(true, true, Some("tmux new -As dev"), None);
        assert!(cmd[2].contains("exec tmux new -As dev"));
    }

    #[test]
    fn fallback_with_dir() {
        let cmd = build_fallback_snippet(true, true, None, Some("~/foo"));
        assert!(cmd[2].starts_with("cd ~/foo && "));
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

    // --- shell-only resolution pipeline ---

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

    // --- resolve routes session targets through backend policy ---

    #[test]
    fn resolve_session_target_surfaces_backend_policy_conflicts() {
        // `--tta` with an explicitly requested classic backend is a policy
        // conflict. resolve must surface the backend policy error rather than
        // silently rendering legacy classic tmux strings (the retired resolve
        // behavior).
        let mut l = launch_with_shell("zsh");
        l.tta = Some("dev".into());
        l.session_backend = SessionBackend::ClassicTmuxCli;
        let error = l.resolve(&[], None).unwrap_err().to_string();
        assert!(
            error.contains("requires a tta backend") || error.contains("was not compiled"),
            "unexpected policy error: {error}"
        );
    }

    #[test]
    fn resolve_session_target_never_yields_classic_renderer_strings() {
        // When policy admits the target, the resolved command is the launcher
        // worker or a tta client — never the legacy `tmux new` /
        // `attach-session` renderer, whatever features this build enables.
        let mut l = launch_with_shell("zsh");
        l.tmux = Some("dev".into());
        if let Ok(spec) = l.resolve(&[], None) {
            assert!(!spec.args.iter().any(|a| a.contains("tmux new")));
            assert!(!spec.args.iter().any(|a| a.contains("attach-session")));
        }
    }
}
