// pattern: Mixed (needs refactoring)
// Functional core: configuration defaults and types.
// Imperative shell: load_config merges external sources.

use std::env;

use bon::Builder;
use clap::Parser;
use figment2::providers::{Env, Format, Serialized, Toml};
use figment2::Figment;
use serde::{Deserialize, Serialize};

use crate::launch::LaunchConfig;

#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct Config {
    #[serde(default = "default_command")]
    pub command: Vec<String>,

    #[serde(default = "default_config_path")]
    pub config: String,

    #[serde(default = "default_forward_signals")]
    pub forward_signals: Vec<String>,

    #[serde(default = "default_shutdown_signals")]
    pub shutdown_signals: Vec<String>,

    #[serde(default = "default_reemit_title")]
    pub reemit_title: bool,

    #[serde(default)]
    pub launch: LaunchConfig,
}

fn default_command() -> Vec<String> {
    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    vec![shell]
}

fn default_config_path() -> String {
    let base = env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
        format!(
            "{}/.config",
            env::var("HOME").unwrap_or_else(|_| "/etc".to_string())
        )
    });
    format!("{base}/title-preserver/config.toml")
}

fn default_forward_signals() -> Vec<String> {
    vec!["SIGTERM".to_string(), "SIGHUP".to_string()]
}

fn default_shutdown_signals() -> Vec<String> {
    Vec::new()
}

fn default_reemit_title() -> bool {
    false
}

impl Default for Config {
    fn default() -> Self {
        Self {
            command: default_command(),
            config: default_config_path(),
            forward_signals: default_forward_signals(),
            shutdown_signals: default_shutdown_signals(),
            reemit_title: default_reemit_title(),
            launch: LaunchConfig::default(),
        }
    }
}

#[derive(Parser)]
#[command(
    name = "title-preserver",
    about = "PTY proxy that observes terminal title sequences"
)]
pub struct CliArgs {
    #[arg(long, help = "Path to config file")]
    pub config: Option<String>,

    #[arg(long, help = "Signals to forward to child")]
    pub forward_signals: Option<Vec<String>>,

    #[arg(
        long,
        help = "Signals that request proxy shutdown after forwarding to an active child"
    )]
    pub shutdown_signals: Option<Vec<String>>,

    #[arg(long, help = "Re-emit captured title on child exit")]
    pub reemit_title: Option<bool>,

    #[arg(long, help = "Shell to run")]
    pub shell: Option<String>,

    #[arg(long, help = "Connect via SSH")]
    pub ssh: Option<String>,

    #[arg(long, help = "Connect via mosh")]
    pub mosh: Option<String>,

    #[arg(long, help = "Create or attach to tmux session")]
    pub tmux: Option<String>,

    #[arg(long, help = "Create tmux session")]
    pub tmux_create: Option<String>,

    #[arg(long, help = "Attach to existing tmux session")]
    pub tmux_attach: Option<String>,

    #[arg(long, help = "Working directory for innermost layer")]
    pub dir: Option<String>,

    #[arg(long, help = "Skip login shell mode")]
    pub no_login: bool,

    #[arg(long, help = "Skip interactive shell mode")]
    pub no_interactive: bool,

    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CliOverlay {
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    forward_signals: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shutdown_signals: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reemit_title: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    launch: Option<LaunchOverlay>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LaunchOverlay {
    #[serde(skip_serializing_if = "Option::is_none")]
    shell: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ssh: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mosh: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tmux: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tmux_create: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tmux_attach: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    login: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    interactive: Option<bool>,
}

impl LaunchOverlay {
    fn is_set(&self) -> bool {
        self.shell.is_some()
            || self.ssh.is_some()
            || self.mosh.is_some()
            || self.tmux.is_some()
            || self.tmux_create.is_some()
            || self.tmux_attach.is_some()
            || self.dir.is_some()
            || self.login.is_some()
            || self.interactive.is_some()
    }
}

impl From<CliArgs> for CliOverlay {
    fn from(args: CliArgs) -> Self {
        let launch_overlay = LaunchOverlay {
            shell: args.shell,
            ssh: args.ssh,
            mosh: args.mosh,
            tmux: args.tmux,
            tmux_create: args.tmux_create,
            tmux_attach: args.tmux_attach,
            dir: args.dir,
            login: if args.no_login { Some(false) } else { None },
            interactive: if args.no_interactive {
                Some(false)
            } else {
                None
            },
        };

        Self {
            command: if args.command.is_empty() {
                None
            } else {
                Some(args.command)
            },
            forward_signals: args.forward_signals,
            shutdown_signals: args.shutdown_signals,
            reemit_title: args.reemit_title,
            launch: if launch_overlay.is_set() {
                Some(launch_overlay)
            } else {
                None
            },
        }
    }
}

pub fn load_config(cli_args: CliArgs) -> eyre::Result<Config> {
    let config_path = cli_args.config.clone().unwrap_or_else(default_config_path);

    let overlay = CliOverlay::from(cli_args);

    let config: Config = Figment::from(Serialized::defaults(Config::default()))
        .merge(Toml::file(&config_path))
        .merge(Env::prefixed("TITLE_PRESERVER_").split("__").ignore_empty(true))
        .merge(Serialized::defaults(overlay))
        .extract()?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::NamedTempFile;

    fn make_cli(args: &[&str]) -> CliArgs {
        CliArgs::try_parse_from(std::iter::once("title-preserver").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn default_command_uses_shell_env() {
        let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let config = Config::default();
        assert_eq!(config.command, vec![shell]);
    }

    #[test]
    fn default_command_falls_back_to_bin_sh() {
        let saved = env::var("SHELL").ok();
        env::remove_var("SHELL");
        let cmd = default_command();
        if let Some(s) = saved {
            env::set_var("SHELL", s);
        }
        assert_eq!(cmd, vec!["/bin/sh"]);
    }

    #[test]
    fn default_forwarded_signals_are_sigterm_and_sighup() {
        let sigs = default_forward_signals();
        assert_eq!(sigs, vec!["SIGTERM", "SIGHUP"]);
    }

    #[test]
    fn default_shutdown_signals_empty_preserves_forward_only() {
        assert!(default_shutdown_signals().is_empty());
    }

    #[test]
    fn cli_overlay_command_replaces_default() {
        let cli = make_cli(&["--config", "/dev/null", "bash", "-l"]);
        let config = load_config(cli).unwrap();
        assert_eq!(config.command, vec!["bash", "-l"]);
    }

    #[test]
    #[serial]
    fn env_overrides_config_file() {
        let mut tmp = NamedTempFile::new().unwrap();
        let config_content = r#"
command = ["/bin/from-file"]
"#;
        use std::io::Write;
        tmp.write_all(config_content.as_bytes()).unwrap();

        let prev_command = env::var("TITLE_PRESERVER_COMMAND").ok();
        env::set_var("TITLE_PRESERVER_COMMAND", r#"["/bin/from-env"]"#);

        let cli = make_cli(&["--config", tmp.path().to_str().unwrap()]);
        let config = load_config(cli).unwrap();

        match prev_command {
            Some(v) => env::set_var("TITLE_PRESERVER_COMMAND", v),
            None => env::remove_var("TITLE_PRESERVER_COMMAND"),
        }

        assert_eq!(config.command, vec!["/bin/from-env"]);
    }

    #[test]
    #[serial]
    fn cli_overrides_env() {
        let prev = env::var("TITLE_PRESERVER_COMMAND").ok();
        env::set_var("TITLE_PRESERVER_COMMAND", r#"["/bin/from-env"]"#);

        let cli = make_cli(&["--config", "/dev/null", "/bin/from-cli"]);
        let config = load_config(cli).unwrap();

        match prev {
            Some(v) => env::set_var("TITLE_PRESERVER_COMMAND", v),
            None => env::remove_var("TITLE_PRESERVER_COMMAND"),
        }

        assert_eq!(config.command, vec!["/bin/from-cli"]);
    }

    #[test]
    fn cli_launch_shell_parses() {
        let cli = make_cli(&["--config", "/dev/null", "--shell", "bash"]);
        let config = load_config(cli).unwrap();
        assert_eq!(config.launch.shell.as_deref(), Some("bash"));
    }

    #[test]
    fn cli_launch_mosh_tmux_parses() {
        let cli = make_cli(&[
            "--config",
            "/dev/null",
            "--mosh",
            "w",
            "--tmux",
            "compfuzor",
        ]);
        let config = load_config(cli).unwrap();
        assert_eq!(config.launch.mosh.as_deref(), Some("w"));
        assert_eq!(config.launch.tmux.as_deref(), Some("compfuzor"));
    }

    #[test]
    fn cli_no_login_maps_to_false() {
        let cli = make_cli(&["--config", "/dev/null", "--no-login"]);
        let config = load_config(cli).unwrap();
        assert!(!config.launch.login);
    }

    #[test]
    fn cli_launch_defaults_login_interactive() {
        let cli = make_cli(&["--config", "/dev/null"]);
        let config = load_config(cli).unwrap();
        assert!(config.launch.login);
        assert!(config.launch.interactive);
    }

    #[test]
    fn cli_launch_not_active_without_flags() {
        let cli = make_cli(&["--config", "/dev/null"]);
        let config = load_config(cli).unwrap();
        assert!(!config.launch.is_active());
    }

    #[test]
    fn cli_launch_active_with_ssh() {
        let cli = make_cli(&["--config", "/dev/null", "--ssh", "w"]);
        let config = load_config(cli).unwrap();
        assert!(config.launch.is_active());
    }
}
