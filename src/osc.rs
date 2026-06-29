// Functional core: OSC title parser with deterministic state machine.
//
// The parser observes one byte at a time, holding state across read-buffer
// boundaries. To support *stripping* title sequences from the byte stream, it
// classifies every byte for the relay:
//
//   - `Emit`: ordinary data — write straight to the output.
//   - `Hold`: the byte may belong to a candidate OSC sequence — append it to a
//     pending buffer until the sequence completes or turns out not to be OSC.
//
// On transitions where a held candidate turns out to be something else (e.g.
// `ESC [` is a CSI, not OSC), the parser sets `flush_pending` so the relay
// drains its pending buffer before handling the current byte. When an OSC
// sequence completes, the parser emits an [`OscCompletion`] carrying the title
// (for codes 0/1/2) or `None` (other codes / overflow). The relay then decides
// whether to drop or flush the pending sequence and whether to record the title.

const TITLE_CAP: usize = 4096;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    OscParam,
    OscData,
    OscEsc,
}

/// What the relay should do with the byte just consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteAction {
    /// Write the byte straight to the output.
    Emit,
    /// Append the byte to the pending buffer; it may belong to a sequence
    /// under inspection.
    Hold,
}

/// Signaled when the byte just consumed terminated an OSC sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OscCompletion {
    /// The captured title for OSC codes 0/1/2; `None` for other codes or when
    /// the data buffer overflowed and the title could not be captured.
    pub title: Option<String>,
}

/// Result of feeding one byte to the observer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByteStep {
    /// If set, the relay should flush its pending buffer to the output before
    /// handling this byte — the buffered bytes are not a filterable sequence.
    pub flush_pending: bool,
    /// What to do with the byte just consumed.
    pub byte: ByteAction,
    /// Set when this byte completed an OSC sequence (it is the terminator).
    pub completed: Option<OscCompletion>,
}

impl ByteStep {
    fn emit() -> Self {
        Self {
            flush_pending: false,
            byte: ByteAction::Emit,
            completed: None,
        }
    }

    fn hold() -> Self {
        Self {
            flush_pending: false,
            byte: ByteAction::Hold,
            completed: None,
        }
    }
}

pub struct TitleObserver {
    state: State,
    osc_param: Vec<u8>,
    osc_data: Vec<u8>,
    osc_overflowed: bool,
    current_title: Option<String>,
}

impl TitleObserver {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            osc_param: Vec::new(),
            osc_data: Vec::new(),
            osc_overflowed: false,
            current_title: None,
        }
    }

    pub fn advance(&mut self, byte: u8) -> ByteStep {
        match self.state {
            State::Ground => {
                if byte == ESC {
                    self.state = State::Esc;
                    ByteStep::hold()
                } else {
                    ByteStep::emit()
                }
            }
            State::Esc => {
                if byte == b']' {
                    self.osc_param.clear();
                    self.osc_data.clear();
                    self.osc_overflowed = false;
                    self.state = State::OscParam;
                    ByteStep::hold()
                } else {
                    // Not an OSC start. Flush the held ESC and re-evaluate this
                    // byte: if it is itself an ESC, begin a fresh candidate.
                    self.reset();
                    if byte == ESC {
                        self.state = State::Esc;
                        ByteStep {
                            flush_pending: true,
                            byte: ByteAction::Hold,
                            completed: None,
                        }
                    } else {
                        ByteStep {
                            flush_pending: true,
                            byte: ByteAction::Emit,
                            completed: None,
                        }
                    }
                }
            }
            State::OscParam => {
                if byte == b';' {
                    self.state = State::OscData;
                    ByteStep::hold()
                } else if byte.is_ascii_digit() {
                    self.osc_param.push(byte);
                    ByteStep::hold()
                } else {
                    self.abort_to_ground(byte)
                }
            }
            State::OscData => {
                if byte == BEL {
                    let title = self.finish_osc();
                    ByteStep {
                        flush_pending: false,
                        byte: ByteAction::Hold,
                        completed: Some(OscCompletion { title }),
                    }
                } else if byte == ESC {
                    self.state = State::OscEsc;
                    ByteStep::hold()
                } else {
                    if self.osc_data.len() < TITLE_CAP {
                        self.osc_data.push(byte);
                    } else {
                        self.osc_overflowed = true;
                    }
                    ByteStep::hold()
                }
            }
            State::OscEsc => {
                if byte == b'\\' {
                    let title = self.finish_osc();
                    ByteStep {
                        flush_pending: false,
                        byte: ByteAction::Hold,
                        completed: Some(OscCompletion { title }),
                    }
                } else {
                    self.abort_to_ground(byte)
                }
            }
        }
    }

    /// The held candidate was not a complete OSC title sequence: drop parser
    /// state to Ground, flush the pending buffer, and re-evaluate `byte` (an
    /// ESC starts a fresh candidate, anything else is emitted).
    fn abort_to_ground(&mut self, byte: u8) -> ByteStep {
        self.reset();
        if byte == ESC {
            self.state = State::Esc;
            ByteStep {
                flush_pending: true,
                byte: ByteAction::Hold,
                completed: None,
            }
        } else {
            ByteStep {
                flush_pending: true,
                byte: ByteAction::Emit,
                completed: None,
            }
        }
    }

    fn finish_osc(&mut self) -> Option<String> {
        if self.osc_overflowed {
            self.reset();
            return None;
        }
        let param = String::from_utf8_lossy(&self.osc_param);
        let is_title = param == "0" || param == "1" || param == "2";
        let result = if is_title {
            let title = String::from_utf8_lossy(&self.osc_data).into_owned();
            self.current_title = Some(title.clone());
            Some(title)
        } else {
            None
        };
        self.reset();
        result
    }

    fn reset(&mut self) {
        self.state = State::Ground;
        self.osc_param.clear();
        self.osc_data.clear();
        self.osc_overflowed = false;
    }

    /// Safety valve for a candidate sequence that never terminates: force the
    /// parser back to Ground so the relay can flush its pending buffer and bound
    /// memory. Only meaningful if the caller is holding bytes.
    pub fn force_reset(&mut self) {
        self.reset();
    }

    #[allow(dead_code)]
    pub fn current_title(&self) -> Option<&str> {
        self.current_title.as_deref()
    }
}

impl Default for TitleObserver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive the observer over `bytes`, collecting completed titles and the
    /// emitted output stream (held bytes are emitted on flush / completion so
    /// the result mirrors what a non-stripping relay would write).
    fn advance_bytes(observer: &mut TitleObserver, bytes: &[u8]) -> Vec<String> {
        let mut titles = Vec::new();
        let mut pending: Vec<u8> = Vec::new();
        for &b in bytes {
            let step = observer.advance(b);
            if step.flush_pending {
                pending.clear();
            }
            match step.byte {
                ByteAction::Emit => {}
                ByteAction::Hold => pending.push(b),
            }
            if let Some(completion) = step.completed {
                pending.clear();
                if let Some(title) = completion.title {
                    titles.push(title);
                }
            }
        }
        titles
    }

    #[test]
    fn osc2_bel_captures_title() {
        let mut obs = TitleObserver::new();
        let titles = advance_bytes(&mut obs, b"\x1b]2;my-title\x07");
        assert_eq!(titles, vec!["my-title".to_string()]);
        assert_eq!(obs.current_title(), Some("my-title"));
    }

    #[test]
    fn osc0_st_captures_title() {
        let mut obs = TitleObserver::new();
        let titles = advance_bytes(&mut obs, b"\x1b]0;title\x1b\\");
        assert_eq!(titles, vec!["title".to_string()]);
        assert_eq!(obs.current_title(), Some("title"));
    }

    #[test]
    fn osc1_captures_icon_title() {
        let mut obs = TitleObserver::new();
        let titles = advance_bytes(&mut obs, b"\x1b]1;icon\x07");
        assert_eq!(titles, vec!["icon".to_string()]);
    }

    #[test]
    fn non_title_osc_does_not_change_title() {
        let mut obs = TitleObserver::new();
        let titles = advance_bytes(&mut obs, b"\x1b]4;color\x07");
        assert!(titles.is_empty());
        assert!(obs.current_title().is_none());
    }

    #[test]
    fn long_title_captured_exactly() {
        let mut obs = TitleObserver::new();
        let long_title: String = "x".repeat(250);
        let input = format!("\x1b]2;{long_title}\x07");
        let titles = advance_bytes(&mut obs, input.as_bytes());
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].len(), 250);
        assert_eq!(titles[0], long_title);
    }

    #[test]
    fn byte_at_a_time_captures_title() {
        let mut obs = TitleObserver::new();
        let bytes = b"\x1b]2;split-title\x07";
        let mut titles = Vec::new();
        let mut pending: Vec<u8> = Vec::new();
        for &b in bytes {
            let step = obs.advance(b);
            if step.flush_pending {
                pending.clear();
            }
            match step.byte {
                ByteAction::Emit => {}
                ByteAction::Hold => pending.push(b),
            }
            if let Some(completion) = step.completed {
                pending.clear();
                if let Some(title) = completion.title {
                    titles.push(title);
                }
            }
        }
        assert_eq!(titles, vec!["split-title".to_string()]);
    }

    #[test]
    fn malformed_osc_does_not_panic() {
        let mut obs = TitleObserver::new();
        advance_bytes(&mut obs, b"\x1b]abc\x07");
        let titles = advance_bytes(&mut obs, b"\x1b]2;ok\x07");
        assert_eq!(titles, vec!["ok".to_string()]);
    }

    #[test]
    fn over_cap_sequence_ignored() {
        let mut obs = TitleObserver::new();
        let long: String = "y".repeat(TITLE_CAP + 100);
        let input = format!("\x1b]2;{long}\x07");
        let titles = advance_bytes(&mut obs, input.as_bytes());
        assert!(titles.is_empty());
        assert!(obs.current_title().is_none());
    }

    #[test]
    fn osc_esc_st_terminates() {
        let mut obs = TitleObserver::new();
        let titles = advance_bytes(&mut obs, b"\x1b]2;test\x1b\\");
        assert_eq!(titles, vec!["test".to_string()]);
    }

    /// A relay that reproduces the stream so we can assert byte dispositions.
    fn relay_stream(observer: &mut TitleObserver, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut pending: Vec<u8> = Vec::new();
        for &b in bytes {
            let step = observer.advance(b);
            if step.flush_pending {
                out.extend_from_slice(&pending);
                pending.clear();
            }
            match step.byte {
                ByteAction::Emit => out.push(b),
                ByteAction::Hold => pending.push(b),
            }
            if let Some(completion) = step.completed {
                // Non-stripping relay: always flush.
                let _ = completion;
                out.extend_from_slice(&pending);
                pending.clear();
            }
        }
        out
    }

    #[test]
    fn non_osc_escape_passes_through() {
        let mut obs = TitleObserver::new();
        // CSI sequence should round-trip byte-for-byte.
        let stream = relay_stream(&mut obs, b"\x1b[31mhi");
        assert_eq!(stream, b"\x1b[31mhi");
    }

    #[test]
    fn plain_text_passes_through() {
        let mut obs = TitleObserver::new();
        let stream = relay_stream(&mut obs, b"hello world");
        assert_eq!(stream, b"hello world");
    }

    #[test]
    fn osc_sequence_round_trips_when_not_stripped() {
        let mut obs = TitleObserver::new();
        let stream = relay_stream(&mut obs, b"beep\x1b]2;t\x07boop");
        assert_eq!(stream, b"beep\x1b]2;t\x07boop");
    }

    #[test]
    fn esc_then_osc_passes_both() {
        let mut obs = TitleObserver::new();
        // A stray ESC immediately followed by an OSC: the lone ESC is flushed,
        // then the OSC is processed normally.
        let stream = relay_stream(&mut obs, b"\x1b\x1b]2;t\x07");
        assert_eq!(stream, b"\x1b\x1b]2;t\x07");
    }
}
