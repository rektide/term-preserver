// Functional core: OSC title parser with deterministic state machine.

const TITLE_CAP: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    OscParam,
    OscData,
    OscEsc,
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

    pub fn advance(&mut self, byte: u8) -> Option<String> {
        match self.state {
            State::Ground => {
                if byte == 0x1b {
                    self.state = State::Esc;
                }
                None
            }
            State::Esc => {
                if byte == b']' {
                    self.osc_param.clear();
                    self.osc_data.clear();
                    self.state = State::OscParam;
                } else {
                    self.state = State::Ground;
                }
                None
            }
            State::OscParam => {
                if byte == b';' {
                    self.state = State::OscData;
                } else if byte == 0x1b || byte == 0x07 {
                    self.reset();
                } else if byte.is_ascii_digit() {
                    self.osc_param.push(byte);
                } else {
                    self.reset();
                }
                None
            }
            State::OscData => match byte {
                0x07 => self.finish_osc(),
                0x1b => {
                    self.state = State::OscEsc;
                    None
                }
                _ => {
                    if self.osc_data.len() < TITLE_CAP {
                        self.osc_data.push(byte);
                    } else {
                        self.osc_overflowed = true;
                    }
                    None
                }
            },
            State::OscEsc => {
                if byte == b'\\' {
                    self.finish_osc()
                } else {
                    self.reset();
                    None
                }
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

    fn advance_bytes(observer: &mut TitleObserver, bytes: &[u8]) -> Vec<String> {
        bytes.iter().filter_map(|&b| observer.advance(b)).collect()
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
        for &b in bytes {
            if let Some(t) = obs.advance(b) {
                titles.push(t);
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
}
