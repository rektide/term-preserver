// pattern: Functional Core
// Terminal color spec parsing for OSC 4 / 10 / 11 / 12 captures.
//
// Terminals accept a few color spec grammars (xterm ctlseqs). We keep the raw
// string verbatim (for faithful round-trip re-emit) and best-effort parse it to
// a normalized 24-bit RGB so the slot carries the actual color value, not just
// an opaque blob.
//
// Supported grammars:
//   rgb:R/G/B    each channel 1-4 hex digits, scaled to 8-bit
//   #RRGGBB      X11 form, 1-4 hex digits per channel (3, 6, 9, or 12 total)
//   rgbi:R/G/B   float channels 0.0..=1.0, scaled to 8-bit
//
// Anything else (named colors, '?', malformed) parses to `rgb: None`.

/// A captured color: the original spec text plus a normalized 24-bit value when
/// the spec was in a recognized grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorSpec {
    pub raw: String,
    pub rgb: Option<[u8; 3]>,
}

impl ColorSpec {
    pub fn new(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        let rgb = parse_color_spec(&raw);
        Self { raw, rgb }
    }
}

/// Parse a terminal color spec into 24-bit RGB. Returns `None` for unrecognized
/// formats or malformed values.
pub fn parse_color_spec(spec: &str) -> Option<[u8; 3]> {
    let spec = spec.trim();
    if let Some(rest) = spec.strip_prefix("rgb:") {
        return parse_slash_channels(rest, parse_hex_channel).or_else(|| {
            // Some terminals emit rgb:RRGGBB (no slashes) as a single 6-hex run.
            parse_hex_run(rest)
        });
    }
    if let Some(rest) = spec.strip_prefix('#') {
        return parse_hex_run(rest);
    }
    if let Some(rest) = spec.strip_prefix("rgbi:") {
        return parse_slash_channels(rest, parse_float_channel);
    }
    None
}

/// Parse `R/G/B` with the given per-channel parser.
fn parse_slash_channels<T>(
    rest: &str,
    parse_chan: impl Fn(&str) -> Option<T>,
) -> Option<[u8; 3]>
where
    T: IntoChannel,
{
    let mut parts = rest.split('/');
    let r = parse_chan(parts.next()?)?;
    let g = parse_chan(parts.next()?)?;
    let b = parse_chan(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some([r.into_u8(), g.into_u8(), b.into_u8()])
}

trait IntoChannel {
    fn into_u8(self) -> u8;
}

impl IntoChannel for u8 {
    fn into_u8(self) -> u8 {
        self
    }
}

/// Parse a hex channel of 1-4 digits and scale to 8-bit.
fn parse_hex_channel(s: &str) -> Option<u8> {
    if s.is_empty() || s.len() > 4 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(s, 16).ok()?;
    Some(scale_to_u8(n, s.len()))
}

/// Parse a contiguous hex run into RGB. Valid total lengths: 3, 6, 9, 12
/// (one, two, three, or four digits per channel).
fn parse_hex_run(s: &str) -> Option<[u8; 3]> {
    if s.is_empty() || s.len() > 12 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let per = s.len() / 3;
    if per == 0 || s.len() != per * 3 {
        return None;
    }
    let chan = |start: usize| parse_hex_channel(&s[start..start + per]);
    Some([chan(0)?, chan(per)?, chan(per * 2)?])
}

/// Parse a float channel `0.0..=1.0` and scale to 8-bit.
fn parse_float_channel(s: &str) -> Option<u8> {
    let v: f32 = s.parse().ok()?;
    if !v.is_finite() || v < 0.0 {
        return None;
    }
    Some((v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Scale a value in `[0, 16^digits - 1]` up to `[0, 255]`.
fn scale_to_u8(value: u32, digits: usize) -> u8 {
    let max = (16u32).checked_pow(digits as u32).map(|m| m - 1).unwrap_or(u32::MAX);
    if max == 0 {
        return 0;
    }
    ((value as u64 * 255 + max as u64 / 2) / max as u64) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_8bit_per_channel() {
        assert_eq!(parse_color_spec("rgb:aa/bb/cc"), Some([0xaa, 0xbb, 0xcc]));
        assert_eq!(parse_color_spec("rgb:00/00/00"), Some([0, 0, 0]));
        assert_eq!(parse_color_spec("rgb:ff/ff/ff"), Some([255, 255, 255]));
    }

    #[test]
    fn rgb_4bit_scales_up() {
        // 'a' = 0xa in a 4-bit space (0..15) scales toward 0xaa-ish.
        let rgb = parse_color_spec("rgb:a/b/c").unwrap();
        assert_eq!(rgb, [0xaa, 0xbb, 0xcc]);
    }

    #[test]
    fn rgb_16bit_truncates_to_8bit() {
        assert_eq!(
            parse_color_spec("rgb:aaaa/bbbb/cccc"),
            Some([0xaa, 0xbb, 0xcc])
        );
    }

    #[test]
    fn hash_short_and_long() {
        assert_eq!(parse_color_spec("#abc"), Some([0xaa, 0xbb, 0xcc]));
        assert_eq!(parse_color_spec("#aabbcc"), Some([0xaa, 0xbb, 0xcc]));
        assert_eq!(parse_color_spec("#000000"), Some([0, 0, 0]));
    }

    #[test]
    fn rgbi_floats() {
        assert_eq!(parse_color_spec("rgbi:1.0/0.0/0.5"), Some([255, 0, 128]));
    }

    #[test]
    fn unknown_returns_none() {
        assert!(parse_color_spec("red").is_none());
        assert!(parse_color_spec("?").is_none());
        assert!(parse_color_spec("rgb:onlytwo").is_none());
        assert!(parse_color_spec("#12").is_none()); // 2 digits isn't 3/6/9/12
    }

    #[test]
    fn color_spec_keeps_raw() {
        let c = ColorSpec::new("#aabbcc");
        assert_eq!(c.raw, "#aabbcc");
        assert_eq!(c.rgb, Some([0xaa, 0xbb, 0xcc]));
    }

    #[test]
    fn color_spec_unparseable_keeps_raw_only() {
        let c = ColorSpec::new("?");
        assert_eq!(c.raw, "?");
        assert!(c.rgb.is_none());
    }
}
