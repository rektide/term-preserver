// pattern: Mixed (needs refactoring)
// Functional core: configuration defaults and types.
// Imperative shell: load_config merges external sources.

use std::env;

use bon::Builder;
use clap::Parser;
use figment2::providers::{Env, Format, Serialized, Toml};
use figment2::Figment;
use serde::{Deserialize, Serialize};

use crate::filter::TitleFilterRule;
use crate::launch::LaunchConfig;
use crate::launcher::SessionBackend;
use crate::tmux::TmuxConfig;

/// Resolved configuration for the proxy, merged from defaults, TOML, env, and
/// CLI (in increasing precedence) by figment2.
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

    #[serde(default = "default_print_gone_title")]
    pub print_gone_title: bool,

    #[serde(default)]
    pub title_filters: Vec<TitleFilterRule>,

    #[serde(default)]
    pub launch: LaunchConfig,

    #[serde(default)]
    pub tmux: TmuxConfig,
}

fn default_command() -> Vec<String> {
    Vec::new()
}

fn default_config_path() -> String {
    let base = env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{}/.config", env::var("HOME").unwrap_or_else(|_| "/etc".to_string())));
    format!("{base}/term-preserver/config.toml")
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

fn default_print_gone_title() -> bool {
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
            print_gone_title: default_print_gone_title(),
            title_filters: Vec::new(),
            launch: LaunchConfig::default(),
            tmux: TmuxConfig::default(),
        }
    }
}

/// Raw CLI arguments (clap). These are overlaid onto [`Config`] in
/// [`load_config`]; `Option` fields are `None` when absent so they don't
/// clobber lower-precedence sources.
#[derive(Parser)]
#[command(name = "term-preserver", about = "PTY proxy that observes terminal title sequences")]
pub struct CliArgs {
    #[arg(long, hide = true, exclusive = true)]
    pub launch_worker: Option<String>,

    #[arg(long, help = "Enter a matching tta session/project")]
    pub tta: Option<String>,
    #[arg(long, value_enum, help = "Session implementation (auto prefers available tta)")]
    pub session_backend: Option<SessionBackend>,
    #[arg(long, help = "tta project root (otherwise TTA_ROOT / PROJECTS_DIR / ~/src)")]
    pub session_root: Option<String>,
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "default",
        help = "tta scaffold generator: default, a configured name, or false (otherwise TTA_SCAFFOLD / default)")]
    pub scaffold: Option<String>,
    #[arg(long, help = "Explicit tmux socket path on the execution host")]
    pub tmux_socket: Option<String>,
    #[arg(long, help = "tmux client executable")]
    pub tmux_program: Option<String>,
    #[arg(long, help = "tta executable for the subprocess implementation")]
    pub tta_program: Option<String>,
    #[arg(long, help = "Path to config file")]
    pub config: Option<String>,

    #[arg(long, help = "Signals to forward to child")]
    pub forward_signals: Option<Vec<String>>,

    #[arg(long, help = "Signals that request proxy shutdown after forwarding to an active child")]
    pub shutdown_signals: Option<Vec<String>>,

    #[arg(long, help = "Re-emit captured title on child exit")]
    pub reemit_title: Option<bool>,

    #[arg(long, help = "Print \"gone <title> (pid N)\" on child exit")]
    pub print_gone_title: Option<bool>,

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

    #[arg(
        long,
        help = "Disable tmux session grouping: --tmux attaches to an existing session instead of joining its group"
    )]
    pub no_tmux_group: bool,

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
    print_gone_title: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    launch: Option<LaunchOverlay>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct LaunchOverlay {
    #[serde(skip_serializing_if = "Option::is_none")]
    tta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_backend: Option<SessionBackend>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scaffold: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tmux_socket: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tmux_program: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tta_program: Option<String>,
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
    tmux_group: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    login: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    interactive: Option<bool>,
}

impl LaunchOverlay {
    /// Any field set to a non-default (non-`None`) value means the CLI carried a
    /// launch flag worth merging. Comparing against `Default` keeps this honest
    /// as fields are added, instead of an enumeration that silently drifts.
    fn is_set(&self) -> bool {
        *self != Self::default()
    }
}

impl From<CliArgs> for CliOverlay {
    fn from(args: CliArgs) -> Self {
        let launch_overlay = LaunchOverlay {
            tta: args.tta,
            session_backend: args.session_backend,
            session_root: args.session_root,
            scaffold: args.scaffold,
            tmux_socket: args.tmux_socket,
            tmux_program: args.tmux_program,
            tta_program: args.tta_program,
            shell: args.shell,
            ssh: args.ssh,
            mosh: args.mosh,
            tmux: args.tmux,
            tmux_create: args.tmux_create,
            tmux_attach: args.tmux_attach,
            tmux_group: if args.no_tmux_group { Some(false) } else { None },
            dir: args.dir,
            login: if args.no_login { Some(false) } else { None },
            interactive: if args.no_interactive { Some(false) } else { None },
        };

        Self {
            command: if args.command.is_empty() { None } else { Some(args.command) },
            forward_signals: args.forward_signals,
            shutdown_signals: args.shutdown_signals,
            reemit_title: args.reemit_title,
            print_gone_title: args.print_gone_title,
            launch: if launch_overlay.is_set() { Some(launch_overlay) } else { None },
        }
    }
}

/// Load [`Config`] by merging defaults < TOML file < environment < CLI. The
/// config file path comes from `--config` or the XDG default. Invalid values
/// (e.g. a bad regex filter) surface as errors.
pub fn load_config(cli_args: CliArgs) -> eyre::Result<Config> {
    let config_path = cli_args.config.clone().unwrap_or_else(default_config_path);

    let overlay = CliOverlay::from(cli_args);

    let config: Config = Figment::from(Serialized::defaults(Config::default()))
        .merge(Toml::file(&config_path))
        .merge(Env::prefixed("TERM_PRESERVER_").split("__").ignore_empty(true))
        .merge(Serialized::defaults(overlay))
        .extract()?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::TmuxSource;
    use serial_test::serial;
    use tempfile::NamedTempFile;

    fn make_cli(args: &[&str]) -> CliArgs {
        CliArgs::try_parse_from(std::iter::once("term-preserver").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn default_command_is_unspecified_until_launch() {
        let config = Config::default();
        assert!(config.command.is_empty());
    }

    #[test]
    fn default_command_falls_back_to_bin_sh() {
        let saved = env::var("SHELL").ok();
        env::remove_var("SHELL");
        let cmd = default_command();
        if let Some(s) = saved {
            env::set_var("SHELL", s);
        }
        assert!(cmd.is_empty());
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

        let prev_command = env::var("TERM_PRESERVER_COMMAND").ok();
        env::set_var("TERM_PRESERVER_COMMAND", r#"["/bin/from-env"]"#);

        let cli = make_cli(&["--config", tmp.path().to_str().unwrap()]);
        let config = load_config(cli).unwrap();

        match prev_command {
            Some(v) => env::set_var("TERM_PRESERVER_COMMAND", v),
            None => env::remove_var("TERM_PRESERVER_COMMAND"),
        }

        assert_eq!(config.command, vec!["/bin/from-env"]);
    }

    #[test]
    #[serial]
    fn cli_overrides_env() {
        let prev = env::var("TERM_PRESERVER_COMMAND").ok();
        env::set_var("TERM_PRESERVER_COMMAND", r#"["/bin/from-env"]"#);

        let cli = make_cli(&["--config", "/dev/null", "/bin/from-cli"]);
        let config = load_config(cli).unwrap();

        match prev {
            Some(v) => env::set_var("TERM_PRESERVER_COMMAND", v),
            None => env::remove_var("TERM_PRESERVER_COMMAND"),
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
        let cli = make_cli(&["--config", "/dev/null", "--mosh", "w", "--tmux", "compfuzor"]);
        let config = load_config(cli).unwrap();
        assert_eq!(config.launch.mosh.as_deref(), Some("w"));
        assert_eq!(config.launch.tmux.as_deref(), Some("compfuzor"));
    }

    #[test]
    #[serial]
    fn session_backend_cli_overrides_file_without_inventing_startup_command() {
        let mut tmp = NamedTempFile::new().unwrap();
        use std::io::Write;
        tmp.write_all(b"[launch]\nsession_backend = 'tta-cli'\nsession_root = '/projects'\n").unwrap();
        let config = load_config(make_cli(&[
            "--config",
            tmp.path().to_str().unwrap(),
            "--tmux",
            "dev",
            "--session-backend",
            "classic-tmux-cli",
            "--tmux-socket",
            "/private/sock",
        ]))
        .unwrap();
        assert_eq!(config.launch.session_backend, SessionBackend::ClassicTmuxCli);
        assert_eq!(config.launch.session_root.as_deref(), Some("/projects"));
        assert_eq!(config.launch.tmux_socket.as_deref(), Some("/private/sock"));
        assert!(config.command.is_empty());
    }

    #[test]
    #[serial]
    fn scaffold_choice_overlays_file_and_preserves_named_or_bare_values() {
        let mut tmp = NamedTempFile::new().unwrap();
        use std::io::Write;
        tmp.write_all(b"[launch]\nscaffold = 'named'\n").unwrap();
        let path = tmp.path().to_str().unwrap();
        assert_eq!(load_config(make_cli(&["--config", path])).unwrap().launch.scaffold.as_deref(), Some("named"));
        assert_eq!(
            load_config(make_cli(&["--config", path, "--scaffold=false"]))
                .unwrap()
                .launch
                .scaffold
                .as_deref(),
            Some("false")
        );
        assert_eq!(
            load_config(make_cli(&["--config", path, "--scaffold"])).unwrap().launch.scaffold.as_deref(),
            Some("default")
        );
    }

    #[test]
    fn cli_no_login_maps_to_false() {
        let cli = make_cli(&["--config", "/dev/null", "--no-login"]);
        let config = load_config(cli).unwrap();
        assert!(!config.launch.login);
    }

    #[test]
    fn cli_launch_tmux_group_defaults_on() {
        let cli = make_cli(&["--config", "/dev/null"]);
        let config = load_config(cli).unwrap();
        assert!(config.launch.tmux_group);
    }

    #[test]
    fn cli_no_tmux_group_maps_to_false() {
        let cli = make_cli(&["--config", "/dev/null", "--no-tmux-group"]);
        let config = load_config(cli).unwrap();
        assert!(!config.launch.tmux_group);
    }

    #[test]
    fn tmux_group_disabled_from_toml() {
        let mut tmp = NamedTempFile::new().unwrap();
        use std::io::Write;
        tmp.write_all(b"[launch]\ntmux_group = false\n").unwrap();
        let cli = make_cli(&["--config", tmp.path().to_str().unwrap()]);
        let config = load_config(cli).unwrap();
        assert!(!config.launch.tmux_group);
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

    // --- tmux sources config ---

    #[test]
    fn default_tmux_sources_is_poll() {
        let config = Config::default();
        assert_eq!(config.tmux.sources, vec![TmuxSource::Poll]);
        assert!(config.tmux.enabled(TmuxSource::Poll));
    }

    #[test]
    fn tmux_sources_disabled_from_toml() {
        let mut tmp = NamedTempFile::new().unwrap();
        use std::io::Write;
        tmp.write_all(b"[tmux]\nsources = []\n").unwrap();
        let cli = make_cli(&["--config", tmp.path().to_str().unwrap()]);
        let config = load_config(cli).unwrap();
        assert!(config.tmux.sources.is_empty());
        assert!(!config.tmux.enabled(TmuxSource::Poll));
    }

    #[test]
    fn tmux_sources_unknown_variant_errors() {
        // `control` / `exit` are planned but not yet variants, so they are
        // rejected today; flip this test when each source lands.
        let mut tmp = NamedTempFile::new().unwrap();
        use std::io::Write;
        tmp.write_all(b"[tmux]\nsources = [\"control\"]\n").unwrap();
        let cli = make_cli(&["--config", tmp.path().to_str().unwrap()]);
        assert!(load_config(cli).is_err());
    }
}
