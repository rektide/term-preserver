// pattern: Mixed (needs refactoring)

use nix::sys::signal::Signal;

use crate::config::Config;

pub fn parse_signal_name(name: &str) -> eyre::Result<Signal> {
    let normalized = name.to_uppercase();
    let without_sig = normalized.strip_prefix("SIG").unwrap_or(&normalized);
    match without_sig {
        "TERM" => Ok(Signal::SIGTERM),
        "HUP" => Ok(Signal::SIGHUP),
        "INT" => Ok(Signal::SIGINT),
        "QUIT" => Ok(Signal::SIGQUIT),
        "USR1" => Ok(Signal::SIGUSR1),
        "USR2" => Ok(Signal::SIGUSR2),
        "WINCH" => Ok(Signal::SIGWINCH),
        other => eyre::bail!("unknown signal: {other}"),
    }
}

pub fn parse_forward_signals(names: &[String]) -> eyre::Result<Vec<Signal>> {
    names.iter().map(|n| parse_signal_name(n)).collect()
}

pub fn parse_shutdown_signals(names: &[String]) -> eyre::Result<Vec<Signal>> {
    names.iter().map(|n| parse_signal_name(n)).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveSignalAction {
    ForwardOnly,
    ForwardThenShutdown,
    Ignore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalPolicy {
    pub forward_signals: Vec<Signal>,
    pub shutdown_signals: Vec<Signal>,
}

impl SignalPolicy {
    pub fn from_config(config: &Config) -> eyre::Result<Self> {
        Ok(Self {
            forward_signals: parse_forward_signals(&config.forward_signals)?,
            shutdown_signals: parse_shutdown_signals(&config.shutdown_signals)?,
        })
    }

    pub fn observed_signals(&self) -> Vec<i32> {
        let mut signals = Vec::new();
        for signal in self
            .forward_signals
            .iter()
            .chain(self.shutdown_signals.iter())
            .chain([Signal::SIGTERM, Signal::SIGHUP].iter())
        {
            let value = *signal as i32;
            if !signals.contains(&value) {
                signals.push(value);
            }
        }
        signals
    }

    pub fn action_for_active_child(&self, signal: Signal) -> ActiveSignalAction {
        if self.shutdown_signals.contains(&signal) {
            ActiveSignalAction::ForwardThenShutdown
        } else if self.forward_signals.contains(&signal) {
            ActiveSignalAction::ForwardOnly
        } else {
            ActiveSignalAction::Ignore
        }
    }

    pub fn should_shutdown_without_child(&self, signal: Signal) -> bool {
        signal == Signal::SIGTERM
            || signal == Signal::SIGHUP
            || self.shutdown_signals.contains(&signal)
            || self.forward_signals.contains(&signal)
    }
}

pub fn forward_to_process_group(child_pid: u32, signal: Signal) -> nix::Result<()> {
    let pid = nix::unistd::Pid::from_raw(child_pid as i32);
    nix::sys::signal::killpg(pid, signal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sigterm_with_prefix() {
        assert_eq!(parse_signal_name("SIGTERM").unwrap(), Signal::SIGTERM);
    }

    #[test]
    fn term_without_prefix() {
        assert_eq!(parse_signal_name("TERM").unwrap(), Signal::SIGTERM);
    }

    #[test]
    fn sighup_accepted() {
        assert_eq!(parse_signal_name("SIGHUP").unwrap(), Signal::SIGHUP);
    }

    #[test]
    fn unknown_returns_error() {
        assert!(parse_signal_name("FAKE").is_err());
    }

    #[test]
    fn custom_list_replaces_defaults() {
        let names: Vec<String> = vec!["SIGUSR1".into()];
        let sigs = parse_forward_signals(&names).unwrap();
        assert_eq!(sigs, vec![Signal::SIGUSR1]);
    }

    #[test]
    fn default_policy_forwards_sigterm_without_shutdown() {
        let config = Config::default();
        let policy = SignalPolicy::from_config(&config).unwrap();
        assert_eq!(
            policy.action_for_active_child(Signal::SIGTERM),
            ActiveSignalAction::ForwardOnly
        );
    }

    #[test]
    fn shutdown_signals_forward_then_shutdown() {
        let config = Config {
            shutdown_signals: vec!["SIGTERM".into()],
            ..Config::default()
        };
        let policy = SignalPolicy::from_config(&config).unwrap();
        assert_eq!(
            policy.action_for_active_child(Signal::SIGTERM),
            ActiveSignalAction::ForwardThenShutdown
        );
    }
}
