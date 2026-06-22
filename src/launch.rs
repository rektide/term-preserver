// pattern: Functional Core

use std::path::Path;

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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LaunchConfig {
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
    #[serde(default = "default_launch_login")]
    pub login: bool,
    #[serde(default = "default_launch_interactive")]
    pub interactive: bool,
}

fn default_launch_login() -> bool {
    true
}

fn default_launch_interactive() -> bool {
    true
}

fn shell_basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
        .to_string()
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

fn build_fallback_snippet(
    login: bool,
    interactive: bool,
    inner: Option<&str>,
    dir: Option<&str>,
) -> Vec<String> {
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

fn resolve_shell(launch: &LaunchConfig) -> ShellResolution {
    if let Some(ref shell) = launch.shell {
        return ShellResolution {
            name: shell.clone(),
            family: detect_shell_family(shell),
            is_fallback: false,
        };
    }

    if let Ok(shell) = std::env::var("SHELL") {
        if !shell.is_empty() {
            let family = detect_shell_family(&shell);
            return ShellResolution {
                name: shell,
                family,
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

fn resolve_inner_command(launch: &LaunchConfig, trailing: &[String]) -> Option<String> {
    let tmux_mode = launch.tmux_mode();

    if let Some(mode) = &tmux_mode {
        return Some(build_tmux_command(mode, launch.dir.as_deref(), trailing));
    }

    if !trailing.is_empty() {
        return Some(trailing.join(" "));
    }

    None
}

fn build_shell_invocation(
    shell: &ShellResolution,
    inner: Option<&str>,
    dir: Option<&str>,
    login: bool,
    interactive: bool,
) -> Vec<String> {
    if shell.is_fallback {
        return build_fallback_snippet(login, interactive, inner, dir);
    }

    let flags = build_flags_string(login, interactive);

    match shell.family {
        ShellFamily::Nushell => {
            let mut cmd = vec![shell.name.clone()];
            match (inner, dir) {
                (None, None) => {}
                (Some(c), None) => {
                    cmd.push("-c".into());
                    cmd.push(c.into());
                }
                (None, Some(d)) => {
                    cmd.push("-c".into());
                    cmd.push(format!("cd {d}; nu"));
                }
                (Some(c), Some(d)) => {
                    cmd.push("-c".into());
                    cmd.push(format!("cd {d}; {c}"));
                }
            }
            cmd
        }
        ShellFamily::Posix | ShellFamily::Fish => {
            let mut cmd = vec![shell.name.clone()];

            let c_arg = match (inner, dir) {
                (None, None) => None,
                (Some(c), None) => Some(format!("exec {c}")),
                (None, Some(d)) => Some(format!(
                    "cd {d} && exec {name} {flags}",
                    name = shell.name,
                    flags = flags
                )),
                (Some(c), Some(d)) => Some(format!("cd {d} && exec {c}")),
            };

            match c_arg {
                Some(c_arg) => {
                    if !flags.is_empty() {
                        cmd.push(flags);
                    }
                    cmd.push("-c".into());
                    cmd.push(c_arg);
                }
                None => {
                    if !flags.is_empty() {
                        cmd.push(flags);
                    }
                }
            }
            cmd
        }
    }
}

fn wrap_transport(cmd: Vec<String>, ssh: Option<&str>, mosh: Option<&str>) -> Vec<String> {
    if let Some(host) = ssh {
        let mut wrapped = vec!["ssh".into(), host.into(), "--".into()];
        wrapped.extend(cmd);
        wrapped
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

    pub fn is_active(&self) -> bool {
        self.shell.is_some()
            || self.ssh.is_some()
            || self.mosh.is_some()
            || self.tmux.is_some()
            || self.tmux_create.is_some()
            || self.tmux_attach.is_some()
            || self.dir.is_some()
            || !self.login
            || !self.interactive
    }

    fn validate(&self) -> eyre::Result<()> {
        if self.ssh.is_some() && self.mosh.is_some() {
            eyre::bail!("--ssh and --mosh are mutually exclusive");
        }

        let tmux_count = [&self.tmux, &self.tmux_create, &self.tmux_attach]
            .iter()
            .filter(|o| o.is_some())
            .count();
        if tmux_count > 1 {
            eyre::bail!("--tmux, --tmux-create, and --tmux-attach are mutually exclusive");
        }

        Ok(())
    }

    pub fn resolve(&self, trailing: &[String]) -> eyre::Result<CommandSpec> {
        self.validate()?;

        let shell = resolve_shell(self);
        let inner = resolve_inner_command(self, trailing);
        let dir_for_shell = if self.tmux_mode().is_some() {
            None
        } else {
            self.dir.as_deref()
        };
        let shell_cmd = build_shell_invocation(
            &shell,
            inner.as_deref(),
            dir_for_shell,
            self.login,
            self.interactive,
        );
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
            login: true,
            interactive: true,
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
        assert_eq!(
            cmd,
            vec!["zsh", "-li", "-c", "cd ~/src/foo && exec zsh -li"]
        );
    }

    #[test]
    fn posix_dir_with_inner() {
        let shell = ShellResolution {
            name: "zsh".into(),
            family: ShellFamily::Posix,
            is_fallback: false,
        };
        let cmd = build_shell_invocation(&shell, Some("make test"), Some("~/src/foo"), true, true);
        assert_eq!(
            cmd,
            vec!["zsh", "-li", "-c", "cd ~/src/foo && exec make test"]
        );
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
        assert_eq!(
            build_tmux_command(&mode, None, &[]),
            "tmux attach-session -t dev"
        );
    }

    #[test]
    fn tmux_with_dir() {
        let mode = TmuxMode::CreateOrAttach("dev".into());
        assert_eq!(
            build_tmux_command(&mode, Some("~/src/foo"), &[]),
            "tmux new -c ~/src/foo -As dev"
        );
    }

    #[test]
    fn tmux_with_trailing() {
        let mode = TmuxMode::CreateOrAttach("dev".into());
        assert_eq!(
            build_tmux_command(&mode, None, &["vim".into()]),
            "tmux new -As dev vim"
        );
    }

    #[test]
    fn tmux_attach_with_dir() {
        let mode = TmuxMode::Attach("dev".into());
        assert_eq!(
            build_tmux_command(&mode, Some("~/foo"), &[]),
            "tmux attach-session -c ~/foo -t dev"
        );
    }

    // --- transport wrapping ---

    #[test]
    fn wrap_ssh() {
        let cmd = wrap_transport(vec!["zsh".into(), "-li".into()], Some("w"), None);
        assert_eq!(cmd, vec!["ssh", "w", "--", "zsh", "-li"]);
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

    // --- full resolution pipeline ---

    #[test]
    fn resolve_local_tmux() {
        let mut l = launch_with_shell("zsh");
        l.tmux = Some("dev".into());
        let spec = l.resolve(&[]).unwrap();
        assert_eq!(spec.program, "zsh");
        assert_eq!(spec.args, vec!["-li", "-c", "exec tmux new -As dev"]);
    }

    #[test]
    fn resolve_mosh_tmux() {
        let mut l = launch_with_shell("zsh");
        l.mosh = Some("w".into());
        l.tmux = Some("compfuzor".into());
        let spec = l.resolve(&[]).unwrap();
        assert_eq!(spec.program, "mosh");
        assert_eq!(
            spec.args,
            vec!["w", "--", "zsh", "-li", "-c", "exec tmux new -As compfuzor"]
        );
    }

    #[test]
    fn resolve_mosh_nu_tmux() {
        let mut l = launch_with_shell("nu");
        l.mosh = Some("w".into());
        l.tmux = Some("compfuzor".into());
        let spec = l.resolve(&[]).unwrap();
        assert_eq!(spec.program, "mosh");
        assert_eq!(
            spec.args,
            vec!["w", "--", "nu", "-c", "tmux new -As compfuzor"]
        );
    }

    #[test]
    fn resolve_tmux_with_dir_and_trailing() {
        let mut l = launch_with_shell("zsh");
        l.tmux = Some("dev".into());
        l.dir = Some("~/src/foo".into());
        let spec = l.resolve(&["vim".into()]).unwrap();
        assert_eq!(spec.program, "zsh");
        assert_eq!(
            spec.args,
            vec!["-li", "-c", "exec tmux new -c ~/src/foo -As dev vim"]
        );
    }

    #[test]
    fn resolve_dir_without_tmux() {
        let mut l = launch_with_shell("zsh");
        l.dir = Some("~/src/foo".into());
        let spec = l.resolve(&[]).unwrap();
        assert_eq!(spec.program, "zsh");
        assert_eq!(spec.args, vec!["-li", "-c", "cd ~/src/foo && exec zsh -li"]);
    }

    #[test]
    fn resolve_dir_with_trailing_no_tmux() {
        let mut l = launch_with_shell("zsh");
        l.dir = Some("~/src/foo".into());
        let spec = l.resolve(&["make".into(), "test".into()]).unwrap();
        assert_eq!(spec.program, "zsh");
        assert_eq!(
            spec.args,
            vec!["-li", "-c", "cd ~/src/foo && exec make test"]
        );
    }

    #[test]
    fn resolve_ssh_without_tmux() {
        let mut l = launch_with_shell("zsh");
        l.ssh = Some("w".into());
        let spec = l.resolve(&[]).unwrap();
        assert_eq!(spec.program, "ssh");
        assert_eq!(spec.args, vec!["w", "--", "zsh", "-li"]);
    }

    #[test]
    fn resolve_bare_shell_override() {
        let l = launch_with_shell("bash");
        let spec = l.resolve(&[]).unwrap();
        assert_eq!(spec.program, "bash");
        assert_eq!(spec.args, vec!["-li"]);
    }

    #[test]
    fn resolve_no_login_no_interactive() {
        let mut l = launch_with_shell("zsh");
        l.login = false;
        l.interactive = false;
        let spec = l.resolve(&[]).unwrap();
        assert_eq!(spec.program, "zsh");
        assert!(spec.args.is_empty());
    }

    #[test]
    fn resolve_errors_on_ssh_and_mosh() {
        let mut l = launch_with_shell("zsh");
        l.ssh = Some("w".into());
        l.mosh = Some("w".into());
        assert!(l.resolve(&[]).is_err());
    }

    #[test]
    fn resolve_errors_on_multiple_tmux_flags() {
        let mut l = launch_with_shell("zsh");
        l.tmux = Some("dev".into());
        l.tmux_attach = Some("dev".into());
        assert!(l.resolve(&[]).is_err());
    }
}
