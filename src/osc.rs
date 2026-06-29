// Functional core: OSC parser with deterministic state machine.
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
// sequence completes, the parser emits an [`OscCompletion`] carrying a typed
// [`OscCapture`] describing which slot changed (or `None` for unrecognized /
// overflowed sequences). The relay then decides whether to drop or flush the
// pending sequence, applies title filters to text captures, and records the
// capture in shared observed state.
//
// Recognized slots (per xterm ctlseqs):
//   OSC 0          -> window title AND icon name (same value)
//   OSC 1          -> icon name
//   OSC 2          -> window title
//   OSC 4;N;SPEC   -> palette entry N (may chain several in one sequence)
//   OSC 10;SPEC    -> default foreground
//   OSC 11;SPEC    -> default background
//   OSC 12;SPEC    -> text cursor color

use std::collections::BTreeMap;

use crate::color::ColorSpec;

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

/// A completed OSC sequence's effect on observed state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OscCapture {
    /// OSC 0 — sets both window title and icon name to the same value.
    Both(String),
    /// OSC 1 — icon name.
    IconName(String),
    /// OSC 2 — window title.
    WindowTitle(String),
    /// OSC 4 — one or more palette entries (index → spec).
    Palette(Vec<(u32, ColorSpec)>),
    /// OSC 5 — one or more special-color entries (index → spec), e.g. bold,
    /// underline, etc. Same grammar as palette.
    Special(Vec<(u32, ColorSpec)>),
    /// OSC 10 — default foreground.
    Foreground(ColorSpec),
    /// OSC 11 — default background.
    Background(ColorSpec),
    /// OSC 12 — text cursor color.
    Cursor(ColorSpec),
    /// OSC 104 — reset palette entries. An empty vec resets the whole palette;
    /// otherwise the listed indices are removed.
    ResetPalette(Vec<u32>),
    /// OSC 105 — reset special-color entries (companion to [`Special`]).
    ResetSpecial(Vec<u32>),
    /// OSC 110 — reset default foreground.
    ResetForeground,
    /// OSC 111 — reset default background.
    ResetBackground,
    /// OSC 112 — reset text cursor color.
    ResetCursor,
}

impl OscCapture {
    /// The title text, if this is a text-bearing capture (window/icon/both).
    /// Used so the relay can apply title filters uniformly.
    pub fn title_text(&self) -> Option<&str> {
        match self {
            OscCapture::Both(v) | OscCapture::IconName(v) | OscCapture::WindowTitle(v) => {
                Some(v)
            }
            _ => None,
        }
    }
}

/// Signaled when the byte just consumed terminated an OSC sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OscCompletion {
    /// The capture, or `None` for unrecognized / overflowed sequences.
    pub capture: Option<OscCapture>,
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
    // Observer's own bookkeeping: the last value seen for each slot. The relay
    // mirrors these into shared observed state (subject to filtering).
    window_title: Option<String>,
    icon_name: Option<String>,
    palette: BTreeMap<u32, ColorSpec>,
    special: BTreeMap<u32, ColorSpec>,
    foreground: Option<ColorSpec>,
    background: Option<ColorSpec>,
    cursor_color: Option<ColorSpec>,
}

impl TitleObserver {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            osc_param: Vec::new(),
            osc_data: Vec::new(),
            osc_overflowed: false,
            window_title: None,
            icon_name: None,
            palette: BTreeMap::new(),
            special: BTreeMap::new(),
            foreground: None,
            background: None,
            cursor_color: None,
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
                } else if byte == BEL {
                    // Param-only termination (e.g. OSC 104 with no args). Finish
                    // with an empty data buffer.
                    let capture = self.finish_osc();
                    ByteStep {
                        flush_pending: false,
                        byte: ByteAction::Hold,
                        completed: Some(OscCompletion { capture }),
                    }
                } else if byte == ESC {
                    self.state = State::OscEsc;
                    ByteStep::hold()
                } else {
                    self.abort_to_ground(byte)
                }
            }
            State::OscData => {
                if byte == BEL {
                    let capture = self.finish_osc();
                    ByteStep {
                        flush_pending: false,
                        byte: ByteAction::Hold,
                        completed: Some(OscCompletion { capture }),
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
                    let capture = self.finish_osc();
                    ByteStep {
                        flush_pending: false,
                        byte: ByteAction::Hold,
                        completed: Some(OscCompletion { capture }),
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

    fn finish_osc(&mut self) -> Option<OscCapture> {
        if self.osc_overflowed {
            self.reset();
            return None;
        }
        let param = String::from_utf8_lossy(&self.osc_param);
        let capture = match param.as_ref() {
            "0" => {
                let value = String::from_utf8_lossy(&self.osc_data).into_owned();
                self.window_title = Some(value.clone());
                self.icon_name = Some(value.clone());
                Some(OscCapture::Both(value))
            }
            "1" => {
                let value = String::from_utf8_lossy(&self.osc_data).into_owned();
                self.icon_name = Some(value.clone());
                Some(OscCapture::IconName(value))
            }
            "2" => {
                let value = String::from_utf8_lossy(&self.osc_data).into_owned();
                self.window_title = Some(value.clone());
                Some(OscCapture::WindowTitle(value))
            }
            "4" => {
                let entries = parse_palette(&self.osc_data);
                for (index, spec) in &entries {
                    self.palette.insert(*index, spec.clone());
                }
                if entries.is_empty() {
                    None
                } else {
                    Some(OscCapture::Palette(entries))
                }
            }
            "5" => {
                let entries = parse_palette(&self.osc_data);
                for (index, spec) in &entries {
                    self.special.insert(*index, spec.clone());
                }
                if entries.is_empty() {
                    None
                } else {
                    Some(OscCapture::Special(entries))
                }
            }
            "10" => {
                let spec = ColorSpec::new(first_segment(&self.osc_data));
                self.foreground = Some(spec.clone());
                Some(OscCapture::Foreground(spec))
            }
            "11" => {
                let spec = ColorSpec::new(first_segment(&self.osc_data));
                self.background = Some(spec.clone());
                Some(OscCapture::Background(spec))
            }
            "12" => {
                let spec = ColorSpec::new(first_segment(&self.osc_data));
                self.cursor_color = Some(spec.clone());
                Some(OscCapture::Cursor(spec))
            }
            "104" => {
                let indices = parse_indices(&self.osc_data);
                if indices.is_empty() {
                    self.palette.clear();
                } else {
                    for i in &indices {
                        self.palette.remove(i);
                    }
                }
                Some(OscCapture::ResetPalette(indices))
            }
            "105" => {
                let indices = parse_indices(&self.osc_data);
                if indices.is_empty() {
                    self.special.clear();
                } else {
                    for i in &indices {
                        self.special.remove(i);
                    }
                }
                Some(OscCapture::ResetSpecial(indices))
            }
            "110" => {
                self.foreground = None;
                Some(OscCapture::ResetForeground)
            }
            "111" => {
                self.background = None;
                Some(OscCapture::ResetBackground)
            }
            "112" => {
                self.cursor_color = None;
                Some(OscCapture::ResetCursor)
            }
            _ => None,
        };
        self.reset();
        capture
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

    pub fn window_title(&self) -> Option<&str> {
        self.window_title.as_deref()
    }

    pub fn icon_name(&self) -> Option<&str> {
        self.icon_name.as_deref()
    }

    pub fn palette(&self) -> &BTreeMap<u32, ColorSpec> {
        &self.palette
    }

    pub fn special(&self) -> &BTreeMap<u32, ColorSpec> {
        &self.special
    }

    pub fn foreground(&self) -> Option<&ColorSpec> {
        self.foreground.as_ref()
    }

    pub fn background(&self) -> Option<&ColorSpec> {
        self.background.as_ref()
    }

    pub fn cursor_color(&self) -> Option<&ColorSpec> {
        self.cursor_color.as_ref()
    }
}

impl Default for TitleObserver {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse OSC 4 palette data (`N;SPEC;N;SPEC;...`) into (index, spec) pairs.
/// Malformed pairs are skipped rather than aborting the whole batch.
fn parse_palette(data: &[u8]) -> Vec<(u32, ColorSpec)> {
    let s = String::from_utf8_lossy(data);
    let mut parts = s.split(';');
    let mut out = Vec::new();
    while let Some(index) = parts.next() {
        if let Some(spec) = parts.next() {
            if let Ok(i) = index.parse::<u32>() {
                if !spec.is_empty() {
                    out.push((i, ColorSpec::new(spec)));
                }
            }
        }
    }
    out
}

/// First `;`-delimited segment of the data, as a String. OSC 10/11/12 take a
/// single color spec; a trailing chained spec is ignored.
fn first_segment(data: &[u8]) -> String {
    let s = String::from_utf8_lossy(data);
    s.split(';').next().unwrap_or_default().to_string()
}

/// Parse OSC 104/105 reset data (`N;N;...`, or empty for "reset all") into a
/// list of indices. Non-numeric tokens are skipped.
fn parse_indices(data: &[u8]) -> Vec<u32> {
    let s = String::from_utf8_lossy(data);
    s.split(';')
        .filter_map(|t| t.trim().parse::<u32>().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive the observer over `bytes`, collecting completed captures.
    fn advance_captures(observer: &mut TitleObserver, bytes: &[u8]) -> Vec<OscCapture> {
        let mut captures = Vec::new();
        for &b in bytes {
            let step = observer.advance(b);
            if let Some(completion) = step.completed {
                if let Some(cap) = completion.capture {
                    captures.push(cap);
                }
            }
        }
        captures
    }

    #[test]
    fn osc2_bel_captures_window_title() {
        let mut obs = TitleObserver::new();
        let caps = advance_captures(&mut obs, b"\x1b]2;my-title\x07");
        assert_eq!(caps, vec![OscCapture::WindowTitle("my-title".into())]);
        assert_eq!(obs.window_title(), Some("my-title"));
        assert!(obs.icon_name().is_none());
    }

    #[test]
    fn osc0_sets_both_window_and_icon() {
        let mut obs = TitleObserver::new();
        let caps = advance_captures(&mut obs, b"\x1b]0;title\x1b\\");
        assert_eq!(caps, vec![OscCapture::Both("title".into())]);
        assert_eq!(obs.window_title(), Some("title"));
        assert_eq!(obs.icon_name(), Some("title"));
    }

    #[test]
    fn osc1_captures_icon_only() {
        let mut obs = TitleObserver::new();
        let caps = advance_captures(&mut obs, b"\x1b]1;icon\x07");
        assert_eq!(caps, vec![OscCapture::IconName("icon".into())]);
        assert_eq!(obs.icon_name(), Some("icon"));
        assert!(obs.window_title().is_none());
    }

    #[test]
    fn osc2_does_not_clobber_icon() {
        let mut obs = TitleObserver::new();
        advance_captures(&mut obs, b"\x1b]1;myicon\x07");
        advance_captures(&mut obs, b"\x1b]2;mywin\x07");
        assert_eq!(obs.icon_name(), Some("myicon"));
        assert_eq!(obs.window_title(), Some("mywin"));
    }

    #[test]
    fn osc1_does_not_clobber_window() {
        let mut obs = TitleObserver::new();
        advance_captures(&mut obs, b"\x1b]2;mywin\x07");
        advance_captures(&mut obs, b"\x1b]1;myicon\x07");
        assert_eq!(obs.window_title(), Some("mywin"));
        assert_eq!(obs.icon_name(), Some("myicon"));
    }

    #[test]
    fn osc4_captures_palette_entry() {
        let mut obs = TitleObserver::new();
        let caps = advance_captures(&mut obs, b"\x1b]4;0;rgb:aa/bb/cc\x07");
        assert_eq!(
            caps,
            vec![OscCapture::Palette(vec![(
                0,
                ColorSpec {
                    raw: "rgb:aa/bb/cc".into(),
                    rgb: Some([0xaa, 0xbb, 0xcc])
                }
            )])]
        );
        // Palette changes do not touch window/icon titles.
        assert!(obs.window_title().is_none());
        assert_eq!(
            obs.palette().get(&0).map(|c| c.rgb),
            Some(Some([0xaa, 0xbb, 0xcc]))
        );
    }

    #[test]
    fn osc4_chains_multiple_palette_entries() {
        let mut obs = TitleObserver::new();
        let caps = advance_captures(&mut obs, b"\x1b]4;1;#abc;2;#def\x07");
        let OscCapture::Palette(entries) = &caps[0] else {
            panic!("expected palette capture");
        };
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, 1);
        assert_eq!(entries[1].0, 2);
        assert_eq!(obs.palette().len(), 2);
    }

    #[test]
    fn osc10_11_12_capture_fg_bg_cursor() {
        let mut obs = TitleObserver::new();
        advance_captures(&mut obs, b"\x1b]10;#112233\x07");
        advance_captures(&mut obs, b"\x1b]11;rgb:00/00/00\x07");
        advance_captures(&mut obs, b"\x1b]12;rgbi:1.0/0.0/0.0\x07");
        assert_eq!(obs.foreground().unwrap().rgb, Some([0x11, 0x22, 0x33]));
        assert_eq!(obs.background().unwrap().rgb, Some([0, 0, 0]));
        assert_eq!(obs.cursor_color().unwrap().rgb, Some([255, 0, 0]));
    }

    #[test]
    fn osc5_captures_special_colors() {
        let mut obs = TitleObserver::new();
        let caps = advance_captures(&mut obs, b"\x1b]5;1;#aabbcc\x07");
        assert_eq!(
            caps,
            vec![OscCapture::Special(vec![(
                1,
                ColorSpec {
                    raw: "#aabbcc".into(),
                    rgb: Some([0xaa, 0xbb, 0xcc])
                }
            )])]
        );
        assert_eq!(obs.special().get(&1).map(|c| c.rgb), Some(Some([0xaa, 0xbb, 0xcc])));
        assert!(obs.palette().is_empty(), "OSC 5 must not touch the palette");
    }

    #[test]
    fn osc104_resets_palette_entries() {
        let mut obs = TitleObserver::new();
        advance_captures(&mut obs, b"\x1b]4;0;#111111;1;#222222\x07");
        assert_eq!(obs.palette().len(), 2);

        // Reset a single entry.
        let caps = advance_captures(&mut obs, b"\x1b]104;0\x07");
        assert_eq!(caps, vec![OscCapture::ResetPalette(vec![0])]);
        assert_eq!(obs.palette().len(), 1);
        assert!(!obs.palette().contains_key(&0));

        // Reset all (param-only, no semicolon).
        let caps = advance_captures(&mut obs, b"\x1b]104\x07");
        assert_eq!(caps, vec![OscCapture::ResetPalette(vec![])]);
        assert!(obs.palette().is_empty());
    }

    #[test]
    fn osc105_resets_special_entries() {
        let mut obs = TitleObserver::new();
        advance_captures(&mut obs, b"\x1b]5;1;#aabbcc\x07");
        assert_eq!(obs.special().len(), 1);
        let caps = advance_captures(&mut obs, b"\x1b]105\x07");
        assert_eq!(caps, vec![OscCapture::ResetSpecial(vec![])]);
        assert!(obs.special().is_empty());
    }

    #[test]
    fn osc110_111_112_reset_fg_bg_cursor() {
        let mut obs = TitleObserver::new();
        advance_captures(&mut obs, b"\x1b]10;#111111\x07");
        advance_captures(&mut obs, b"\x1b]11;#222222\x07");
        advance_captures(&mut obs, b"\x1b]12;#333333\x07");
        assert!(obs.foreground().is_some());

        let caps = advance_captures(&mut obs, b"\x1b]110\x07");
        assert_eq!(caps, vec![OscCapture::ResetForeground]);
        assert!(obs.foreground().is_none());
        assert!(obs.background().is_some());

        advance_captures(&mut obs, b"\x1b]111\x07");
        assert!(obs.background().is_none());
        advance_captures(&mut obs, b"\x1b]112\x07");
        assert!(obs.cursor_color().is_none());
    }

    #[test]
    fn param_only_osc_terminates_with_bel() {
        // OSC 104 with no args is a bare param followed by BEL — it must
        // complete, not abort.
        let mut obs = TitleObserver::new();
        let caps = advance_captures(&mut obs, b"\x1b]104\x07");
        assert_eq!(caps, vec![OscCapture::ResetPalette(vec![])]);
    }

    #[test]
    fn param_only_osc_terminates_with_st() {
        let mut obs = TitleObserver::new();
        let caps = advance_captures(&mut obs, b"\x1b]110\x1b\\");
        assert_eq!(caps, vec![OscCapture::ResetForeground]);
    }

    #[test]
    fn unrecognized_osc_yields_no_capture() {
        let mut obs = TitleObserver::new();
        let caps = advance_captures(&mut obs, b"\x1b]52;c;base64\x07");
        assert!(caps.is_empty());
        assert!(obs.window_title().is_none());
        assert!(obs.foreground().is_none());
    }

    #[test]
    fn long_title_captured_exactly() {
        let mut obs = TitleObserver::new();
        let long_title: String = "x".repeat(250);
        let input = format!("\x1b]2;{long_title}\x07");
        let caps = advance_captures(&mut obs, input.as_bytes());
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0], OscCapture::WindowTitle(long_title));
    }

    #[test]
    fn byte_at_a_time_captures_title() {
        let mut obs = TitleObserver::new();
        let bytes = b"\x1b]2;split-title\x07";
        let mut caps = Vec::new();
        for &b in bytes {
            let step = obs.advance(b);
            if let Some(completion) = step.completed {
                if let Some(cap) = completion.capture {
                    caps.push(cap);
                }
            }
        }
        assert_eq!(caps, vec![OscCapture::WindowTitle("split-title".into())]);
    }

    #[test]
    fn malformed_osc_does_not_panic() {
        let mut obs = TitleObserver::new();
        advance_captures(&mut obs, b"\x1b]abc\x07");
        let caps = advance_captures(&mut obs, b"\x1b]2;ok\x07");
        assert_eq!(caps, vec![OscCapture::WindowTitle("ok".into())]);
    }

    #[test]
    fn over_cap_sequence_yields_no_capture() {
        let mut obs = TitleObserver::new();
        let long: String = "y".repeat(TITLE_CAP + 100);
        let input = format!("\x1b]2;{long}\x07");
        let caps = advance_captures(&mut obs, input.as_bytes());
        assert!(caps.is_empty());
        assert!(obs.window_title().is_none());
    }

    #[test]
    fn osc_esc_st_terminates() {
        let mut obs = TitleObserver::new();
        let caps = advance_captures(&mut obs, b"\x1b]2;test\x1b\\");
        assert_eq!(caps, vec![OscCapture::WindowTitle("test".into())]);
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
            if step.completed.is_some() {
                // Non-stripping relay: always flush.
                out.extend_from_slice(&pending);
                pending.clear();
            }
        }
        out
    }

    #[test]
    fn non_osc_escape_passes_through() {
        let mut obs = TitleObserver::new();
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
        let stream = relay_stream(&mut obs, b"\x1b\x1b]2;t\x07");
        assert_eq!(stream, b"\x1b\x1b]2;t\x07");
    }
}
