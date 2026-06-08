// Imperative shell: signal parsing and forwarding to child process group.

use nix::sys::signal::Signal;

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
}
