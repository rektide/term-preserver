// pattern: Functional Core
// Title filtering: rules that decide how a captured title is handled.
//
// Each rule carries two independent actions:
//   - `strip`:  drop the OSC title sequence from the PTY→stdout byte stream so
//               the *outer* terminal never has its title set to the match.
//   - `ignore`: leave the proxy's internal title state untouched (so a later
//               re-emit won't reproduce it).
//
// A title matching several rules ORs their actions together.
//
// ## The `regex` feature
//
// Matching is gated behind the `regex` Cargo feature (on by default). With it
// disabled, [`FilterSet`] becomes a no-op: [`FilterSet::compile`] accepts the
// rules but discards them, and [`FilterSet::decide`] always passes. This lets a
// consumer build without the `regex` dependency when filtering is not needed.

use serde::{Deserialize, Serialize};

/// A single filter rule as it appears in config.
///
/// `pattern` is matched against the title. With the `regex` feature it is a
/// regular expression (unanchored search); without the feature it is ignored.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TitleFilterRule {
    /// Regex matched against the title (unanchored search; anchor with `^`/`$`).
    pub pattern: String,
    /// Remove the matching OSC sequence from the output byte stream.
    #[serde(default)]
    pub strip: bool,
    /// Do not record the matching title in the proxy's internal title state.
    #[serde(default)]
    pub ignore: bool,
}

/// The outcome of matching a title against a [`FilterSet`]. Actions from every
/// matching rule are OR-ed together.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FilterDecision {
    pub strip: bool,
    pub ignore: bool,
}

impl FilterDecision {
    /// `true` when no rule matched or every match left both flags clear, i.e.
    /// the title should pass through unchanged and be recorded normally.
    pub fn passes(self) -> bool {
        !self.strip && !self.ignore
    }
}

/// A compiled, match-ready set of title-filter rules.
///
/// Construct with [`FilterSet::compile`] (or [`FilterSet::empty`]). Clones are
/// cheap and intended for sharing across relay threads.
#[derive(Debug, Clone, Default)]
pub struct FilterSet {
    #[cfg(feature = "regex")]
    rules: Vec<CompiledRule>,
}

#[cfg(feature = "regex")]
#[derive(Debug, Clone)]
struct CompiledRule {
    regex: regex::Regex,
    strip: bool,
    ignore: bool,
}

impl FilterSet {
    /// An empty set that matches nothing (every title passes).
    pub fn empty() -> Self {
        Self {
            #[cfg(feature = "regex")]
            rules: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        #[cfg(feature = "regex")]
        {
            self.rules.is_empty()
        }
        #[cfg(not(feature = "regex"))]
        {
            true
        }
    }

    /// Compile a config rule list.
    ///
    /// With the `regex` feature this compiles each pattern and fails on the
    /// first invalid regex. Without the feature the rules are accepted but
    /// discarded (filtering is disabled), so this never fails.
    pub fn compile(rules: &[TitleFilterRule]) -> eyre::Result<Self> {
        #[cfg(feature = "regex")]
        {
            use eyre::WrapErr;
            let mut compiled = Vec::with_capacity(rules.len());
            for rule in rules {
                let regex = regex::Regex::new(&rule.pattern).wrap_err_with(|| {
                    format!("compiling title filter regex: {:?}", rule.pattern)
                })?;
                compiled.push(CompiledRule {
                    regex,
                    strip: rule.strip,
                    ignore: rule.ignore,
                });
            }
            Ok(Self { rules: compiled })
        }
        #[cfg(not(feature = "regex"))]
        {
            let _ = rules;
            Ok(Self::empty())
        }
    }

    /// Combine every rule that matches `title` into a single decision. Without
    /// the `regex` feature this always returns a passing (default) decision.
    pub fn decide(&self, title: &str) -> FilterDecision {
        #[cfg(feature = "regex")]
        {
            let mut decision = FilterDecision::default();
            for rule in &self.rules {
                if rule.regex.is_match(title) {
                    decision.strip |= rule.strip;
                    decision.ignore |= rule.ignore;
                }
            }
            decision
        }
        #[cfg(not(feature = "regex"))]
        {
            let _ = title;
            FilterDecision::default()
        }
    }
}

#[cfg(all(test, feature = "regex"))]
mod tests {
    use super::*;

    fn rule(pattern: &str, strip: bool, ignore: bool) -> TitleFilterRule {
        TitleFilterRule {
            pattern: pattern.into(),
            strip,
            ignore,
        }
    }

    #[test]
    fn empty_set_passes_everything() {
        let set = FilterSet::empty();
        assert!(set.decide("rektide@host").passes());
        assert!(set.is_empty());
    }

    #[test]
    fn strip_rule_decides_strip() {
        let set = FilterSet::compile(&[rule("^rektide@", true, false)]).unwrap();
        let d = set.decide("rektide@mybox");
        assert!(d.strip);
        assert!(!d.ignore);
    }

    #[test]
    fn ignore_rule_decides_ignore() {
        let set = FilterSet::compile(&[rule("user@host$", false, true)]).unwrap();
        let d = set.decide("alice@user@host");
        assert!(!d.strip);
        assert!(d.ignore);
    }

    #[test]
    fn non_match_passes() {
        let set = FilterSet::compile(&[rule("^rektide@", true, true)]).unwrap();
        assert!(set.decide("someoneelse@host").passes());
    }

    #[test]
    fn multiple_rules_or_actions() {
        let set = FilterSet::compile(&[
            rule("^rektide@", true, false),
            rule("@mybox$", false, true),
        ])
        .unwrap();
        let d = set.decide("rektide@mybox");
        assert!(d.strip);
        assert!(d.ignore);
    }

    #[test]
    fn unanchored_regex_matches_substring() {
        let set = FilterSet::compile(&[rule("detached", false, true)]).unwrap();
        assert!(set.decide("[d] tmux: detached session").ignore);
    }

    #[test]
    fn compile_reports_bad_regex() {
        let err = FilterSet::compile(&[rule("(unclosed", false, true)]);
        assert!(err.is_err());
    }
}
