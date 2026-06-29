// pattern: Functional Core
// Title filtering: regex rules that decide how a captured title is handled.
//
// Each rule carries two independent actions:
//   - `strip`:  drop the OSC title sequence from the PTY→stdout byte stream so
//               the *outer* terminal never has its title set to the match.
//   - `ignore`: leave the proxy's internal title state untouched (so a later
//               re-emit won't reproduce it).
//
// A title matching several rules ORs their actions together.

use eyre::WrapErr;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// A single filter rule as it appears in config.
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

/// Compiled, match-ready form of a [`TitleFilterRule`].
#[derive(Debug, Clone)]
struct CompiledRule {
    regex: Regex,
    strip: bool,
    ignore: bool,
}

/// The outcome of matching a title against a [`FilterSet`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FilterDecision {
    pub strip: bool,
    pub ignore: bool,
}

impl FilterDecision {
    pub fn passes(self) -> bool {
        !self.strip && !self.ignore
    }
}

/// A compiled set of title-filter rules, cheap to share across relay threads.
#[derive(Debug, Clone, Default)]
pub struct FilterSet {
    rules: Vec<CompiledRule>,
}

impl FilterSet {
    pub fn empty() -> Self {
        Self { rules: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Compile a config rule list. Returns the first regex error encountered.
    pub fn compile(rules: &[TitleFilterRule]) -> eyre::Result<Self> {
        let mut compiled = Vec::with_capacity(rules.len());
        for rule in rules {
            let regex = Regex::new(&rule.pattern).wrap_err_with(|| {
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

    /// Combine every rule that matches `title` into a single decision.
    pub fn decide(&self, title: &str) -> FilterDecision {
        let mut decision = FilterDecision::default();
        for rule in &self.rules {
            if rule.regex.is_match(title) {
                decision.strip |= rule.strip;
                decision.ignore |= rule.ignore;
            }
        }
        decision
    }
}

#[cfg(test)]
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
