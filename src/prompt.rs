// pattern: Functional Core

use crate::session::{ObservedSlots, SessionExit};
use crate::tmux::TmuxSample;

/// The retry-prompt policy decision for a keystroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptDecision {
    Retry,
    Exit(i32),
    Ignore,
}

/// Stateless retry-prompt policy. Maps a prompt keystroke to a [`PromptDecision`]
/// and formats the status prompt. Pure and testable without spawning the binary.
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

/// Format the post-exit prompt shown to the user: a leading CRLF then the
/// exit code and the `space=retry q=quit` hint.
pub fn format_status_prompt(exit_code: i32) -> String {
    format!("\r\n[term-preserver] child exited with code {exit_code}; space=retry q=quit")
}

/// Reconstruct OSC sequences from captured state so the outer terminal's
/// title/icon/color can be restored on child exit. Uses the raw color specs so
/// the round trip is faithful. Emits OSC 1 (icon), OSC 2 (window title),
/// OSC 4 (palette, chained), OSC 10/11/12 (fg/bg/cursor).
pub fn reemit_sequences(slots: &ObservedSlots) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(icon) = &slots.icon_name {
        out.extend(format!("\x1b]1;{icon}\x07").bytes());
    }
    if let Some(title) = &slots.window_title {
        out.extend(format!("\x1b]2;{title}\x07").bytes());
    }
    if !slots.palette.is_empty() {
        let mut body = String::from("4");
        for (index, spec) in &slots.palette {
            body.push_str(&format!(";{index};{}", spec.raw));
        }
        out.extend(format!("\x1b]{body}\x07").bytes());
    }
    if !slots.special.is_empty() {
        let mut body = String::from("5");
        for (index, spec) in &slots.special {
            body.push_str(&format!(";{index};{}", spec.raw));
        }
        out.extend(format!("\x1b]{body}\x07").bytes());
    }
    if let Some(spec) = &slots.foreground {
        out.extend(format!("\x1b]10;{}\x07", spec.raw).bytes());
    }
    if let Some(spec) = &slots.background {
        out.extend(format!("\x1b]11;{}\x07", spec.raw).bytes());
    }
    if let Some(spec) = &slots.cursor_color {
        out.extend(format!("\x1b]12;{}\x07", spec.raw).bytes());
    }
    out
}

/// A visible, human-readable "gone" line for the captured window title and pid,
/// printed at the exit boundary so the user can see what title the exiting
/// process owned. When a tmux sample is present, the session id is appended.
/// Distinct from [`reemit_sequences`], which is invisible.
pub fn format_gone_line(
    slots: &ObservedSlots,
    child_pid: u32,
    tmux: Option<&TmuxSample>,
) -> Option<String> {
    let title = slots.window_title.as_deref()?;
    let suffix = match tmux.and_then(|s| s.session_id.as_deref()) {
        Some(sid) => format!("pid {child_pid}, tmux {sid}"),
        None => format!("pid {child_pid}"),
    };
    Some(format!("\r\n[term-preserver] gone: {title} ({suffix})"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::ColorSpec;
    use std::collections::BTreeMap;

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
    fn reemit_window_and_icon() {
        let mut slots = ObservedSlots {
            window_title: Some("win".into()),
            icon_name: Some("ico".into()),
            ..Default::default()
        };
        let seq = reemit_sequences(&slots);
        let s = String::from_utf8(seq).unwrap();
        assert!(s.contains("\x1b]1;ico\x07"));
        assert!(s.contains("\x1b]2;win\x07"));

        slots.icon_name = None;
        let seq = reemit_sequences(&slots);
        assert!(!String::from_utf8(seq).unwrap().contains("\x1b]1;"));
    }

    #[test]
    fn reemit_palette_chains_entries() {
        let mut palette = BTreeMap::new();
        palette.insert(0, ColorSpec::new("#aabbcc"));
        palette.insert(1, ColorSpec::new("rgb:00/00/00"));
        let slots = ObservedSlots {
            palette,
            ..Default::default()
        };
        let seq = reemit_sequences(&slots);
        let s = String::from_utf8(seq).unwrap();
        assert!(s.contains("\x1b]4;0;#aabbcc;1;rgb:00/00/00\x07"));
    }

    #[test]
    fn reemit_special_colors() {
        let mut special = BTreeMap::new();
        special.insert(1, ColorSpec::new("#abcdef"));
        let slots = ObservedSlots {
            special,
            ..Default::default()
        };
        let s = String::from_utf8(reemit_sequences(&slots)).unwrap();
        assert!(s.contains("\x1b]5;1;#abcdef\x07"));
        assert!(
            !s.contains("\x1b]4;"),
            "palette re-emit should be absent when only special colors are set"
        );
    }

    #[test]
    fn reemit_fg_bg_cursor() {
        let slots = ObservedSlots {
            foreground: Some(ColorSpec::new("#111111")),
            background: Some(ColorSpec::new("#222222")),
            cursor_color: Some(ColorSpec::new("#333333")),
            ..Default::default()
        };
        let s = String::from_utf8(reemit_sequences(&slots)).unwrap();
        assert!(s.contains("\x1b]10;#111111\x07"));
        assert!(s.contains("\x1b]11;#222222\x07"));
        assert!(s.contains("\x1b]12;#333333\x07"));
    }

    #[test]
    fn gone_line_uses_window_title_and_pid() {
        let slots = ObservedSlots {
            window_title: Some("user@host".into()),
            ..Default::default()
        };
        let line = format_gone_line(&slots, 4242, None).unwrap();
        assert!(line.contains("gone: user@host"));
        assert!(line.contains("pid 4242"));
    }

    #[test]
    fn gone_line_appends_tmux_session_id() {
        let slots = ObservedSlots {
            window_title: Some("user@host".into()),
            ..Default::default()
        };
        let sample = TmuxSample {
            session_id: Some("$3".into()),
            ..Default::default()
        };
        let line = format_gone_line(&slots, 4242, Some(&sample)).unwrap();
        assert!(line.contains("tmux $3"));
    }

    #[test]
    fn gone_line_none_without_title() {
        let slots = ObservedSlots::default();
        assert!(format_gone_line(&slots, 1, None).is_none());
    }

    #[test]
    fn gone_line_none_when_only_icon_or_color_set() {
        // The gone line is keyed on the window title; icon or colors alone
        // don't produce one.
        let slots = ObservedSlots {
            icon_name: Some("ico".into()),
            foreground: Some(ColorSpec::new("#123456")),
            ..Default::default()
        };
        assert!(format_gone_line(&slots, 7, None).is_none());
    }

    #[test]
    fn reemit_full_state_round_trips_every_slot_kind() {
        let mut palette = BTreeMap::new();
        palette.insert(8, ColorSpec::new("#001122"));
        let mut special = BTreeMap::new();
        special.insert(1, ColorSpec::new("rgb:aa/bb/cc"));
        let slots = ObservedSlots {
            window_title: Some("win".into()),
            icon_name: Some("ico".into()),
            palette,
            special,
            foreground: Some(ColorSpec::new("#111111")),
            background: Some(ColorSpec::new("#222222")),
            cursor_color: Some(ColorSpec::new("#333333")),
        };
        let s = String::from_utf8(reemit_sequences(&slots)).unwrap();
        assert!(s.contains("\x1b]1;ico\x07"));
        assert!(s.contains("\x1b]2;win\x07"));
        assert!(s.contains("\x1b]4;8;#001122\x07"));
        assert!(s.contains("\x1b]5;1;rgb:aa/bb/cc\x07"));
        assert!(s.contains("\x1b]10;#111111\x07"));
        assert!(s.contains("\x1b]11;#222222\x07"));
        assert!(s.contains("\x1b]12;#333333\x07"));
    }

    #[test]
    fn reemit_empty_state_emits_nothing() {
        assert!(reemit_sequences(&ObservedSlots::default()).is_empty());
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
            child_pid: 1234,
            observed: ObservedSlots::default(),
            tmux: None,
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
