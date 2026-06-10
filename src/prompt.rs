// pattern: Functional Core

use crate::session::SessionExit;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptDecision {
    Retry,
    Exit(i32),
    Ignore,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PromptPolicy;

impl PromptPolicy {
    pub fn format_prompt(&self, exit: &SessionExit) -> String {
        format_status_prompt(exit.exit_code)
    }

    pub fn decide_key(&self, byte: u8, _exit: &SessionExit) -> PromptDecision {
        match byte {
            b' ' => PromptDecision::Retry,
            b'q' => PromptDecision::Exit(0),
            0x03 => PromptDecision::Exit(130),
            _ => PromptDecision::Ignore,
        }
    }
}

pub fn format_status_prompt(exit_code: i32) -> String {
    format!("\r\n[title-proxy] child exited with code {exit_code}; space=retry q=quit")
}

pub fn title_reemit_sequence(title: &str) -> Vec<u8> {
    format!("\x1b]2;{title}\x07").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_includes_exit_code() {
        let p = format_status_prompt(7);
        assert!(p.contains("code 7"));
        assert!(p.contains("space=retry q=quit"));
    }

    #[test]
    fn prompt_starts_with_crlf() {
        let p = format_status_prompt(0);
        assert!(p.starts_with("\r\n"));
    }

    #[test]
    fn title_reemit_sequence_format() {
        let seq = title_reemit_sequence("my-title");
        assert_eq!(seq, b"\x1b]2;my-title\x07");
    }

    #[test]
    fn prompt_policy_maps_keys() {
        let exit = SessionExit {
            command: crate::session::CommandSpec {
                program: "sh".into(),
                args: Vec::new(),
            },
            exit_code: 7,
            raw_status: std::process::Command::new("sh")
                .arg("-c")
                .arg("exit 7")
                .status()
                .unwrap(),
            last_title: None,
            relay_report: crate::session::RelayJoinReport {
                stdin_to_pty: crate::session::RelayEnd::Completed(0),
                pty_to_stdout: crate::session::RelayEnd::Completed(0),
            },
        };
        let policy = PromptPolicy;
        assert_eq!(policy.decide_key(b' ', &exit), PromptDecision::Retry);
        assert_eq!(policy.decide_key(b'q', &exit), PromptDecision::Exit(0));
        assert_eq!(policy.decide_key(0x03, &exit), PromptDecision::Exit(130));
        assert_eq!(policy.decide_key(b'x', &exit), PromptDecision::Ignore);
    }
}
