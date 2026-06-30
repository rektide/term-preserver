//! tmux session introspection.
//!
//! When the proxy launches a *local* tmux session, a [`TmuxSampler`] polls the
//! local tmux server every ~20s, correlates the spawned tmux client to its
//! session by PID, and records a [`TmuxSample`] into a thread-shared
//! [`TmuxState`]. The session snapshots that state on child exit, so the exit
//! dump carries the live session id and the per-pane inner command even though
//! the tmux client (and possibly the server) is gone by then — the cached last
//! sample survives.
//!
//! Parsing is split from I/O: the `parse_*` / `find_*` functions are pure and
//! unit-tested; only the `query_*` / `sample_once` / [`TmuxSampler`] shell out.
//!
//! ## Feature gating
//!
//! The sourcing machinery (subprocess queries, the sampler thread) lives behind
//! the `tmux` Cargo feature. The slot types — [`TmuxSample`], [`TmuxPaneInfo`],
//! [`TmuxState`] — are always present so the rest of the system (the `tmux`
//! field on `SessionExit`, the formatters) is identical either way. With the
//! feature off, [`TmuxSampler::start`] is a no-op stub: no thread, no `tmux`
//! subprocess, and the slot stays empty. The slot is source-agnostic by design,
//! so additional sources (control mode, exit-time query) can write to
//! [`TmuxState`] and be toggled independently.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::launch::TmuxMode;

#[cfg(feature = "tmux")]
use std::process::Command;
#[cfg(feature = "tmux")]
use std::thread;
#[cfg(feature = "tmux")]
use std::time::Duration;

/// One pane's introspected state, as reported by `tmux list-panes`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TmuxPaneInfo {
    pub window_index: String,
    pub window_name: String,
    pub pane_id: String,
    pub current_command: String,
    pub current_path: String,
    pub pane_pid: Option<u32>,
}

/// A snapshot of the tmux session the proxy launched: its id/name and the
/// panes (one entry per window/pane in the session).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TmuxSample {
    pub session_id: Option<String>,
    pub session_name: Option<String>,
    pub panes: Vec<TmuxPaneInfo>,
}

/// Thread-shared holder for the latest [`TmuxSample`]. The sampler writes; the
/// session reads a snapshot on child exit. Mirrors [`crate::session::ObservedState`].
#[derive(Debug, Clone)]
pub struct TmuxState {
    inner: Arc<Mutex<Option<TmuxSample>>>,
}

impl TmuxState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
        }
    }

    /// Record a fresh sample ("set").
    pub fn set(&self, sample: TmuxSample) {
        if let Ok(mut guard) = self.inner.lock() {
            *guard = Some(sample);
        }
    }

    pub fn snapshot(&self) -> Option<TmuxSample> {
        self.inner.lock().ok().and_then(|guard| guard.clone())
    }
}

impl Default for TmuxState {
    fn default() -> Self {
        Self::new()
    }
}

// --- pure parsing -------------------------------------------------------

/// Parse one `list-clients` line: `pid \t session_id \t session_name`.
pub fn parse_client_line(line: &str) -> Option<(u32, String, String)> {
    let mut parts = line.split('\t');
    let pid = parts.next()?.parse::<u32>().ok()?;
    let session_id = parts.next()?.to_string();
    let session_name = parts.next()?.to_string();
    Some((pid, session_id, session_name))
}

/// Over `list-clients` output lines, find the session attached to `child_pid`.
/// Returns `(session_id, session_name)`.
pub fn find_session_for_pid<'a>(
    lines: impl IntoIterator<Item = &'a str>,
    child_pid: u32,
) -> Option<(String, String)> {
    for line in lines {
        if let Some((pid, session_id, session_name)) = parse_client_line(line) {
            if pid == child_pid {
                return Some((session_id, session_name));
            }
        }
    }
    None
}

/// Parse one `list-sessions` line: `session_name \t session_id`.
pub fn parse_session_line(line: &str) -> Option<(String, String)> {
    let mut parts = line.split('\t');
    let name = parts.next()?.to_string();
    let id = parts.next()?.to_string();
    Some((name, id))
}

/// Parse one `list-panes` line into a [`TmuxPaneInfo`].
pub fn parse_pane_line(line: &str) -> Option<TmuxPaneInfo> {
    let mut parts = line.split('\t');
    Some(TmuxPaneInfo {
        window_index: parts.next()?.to_string(),
        window_name: parts.next()?.to_string(),
        pane_id: parts.next()?.to_string(),
        current_command: parts.next()?.to_string(),
        current_path: parts.next()?.to_string(),
        pane_pid: parts.next().and_then(|s| s.parse().ok()),
    })
}

// --- subprocess queries (behind the `tmux` feature) ---------------------

/// How often the sampler refreshes a sample.
#[cfg(feature = "tmux")]
const SAMPLE_INTERVAL: Duration = Duration::from_secs(20);
/// Poll granularity while sleeping between samples, so the sampler notices a
/// dead child promptly instead of blocking for the whole interval.
#[cfg(feature = "tmux")]
const POLL_TICK: Duration = Duration::from_millis(100);

/// Run `tmux <args>` and return its stdout as text, or `None` if tmux is absent
/// or reports failure (e.g. no server / no sessions).
#[cfg(feature = "tmux")]
fn tmux_output(args: &[&str]) -> Option<String> {
    let output = Command::new("tmux").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Authoritative correlation: the spawned child is a tmux client, so match its
/// PID against `#{client_pid}`. Only valid while the client is attached.
#[cfg(feature = "tmux")]
fn query_session_for_pid(child_pid: u32) -> Option<(String, String)> {
    let text = tmux_output(&[
        "list-clients",
        "-F",
        "#{client_pid}\t#{session_id}\t#{session_name}",
    ])?;
    find_session_for_pid(text.lines(), child_pid)
}

/// Fallback correlation by the session name we asked for (`--tmux`). Weaker:
/// tmux may auto-rename or host a duplicate name.
#[cfg(feature = "tmux")]
fn query_session_by_name(name: &str) -> Option<(String, String)> {
    let text = tmux_output(&["list-sessions", "-F", "#{session_name}\t#{session_id}"])?;
    for line in text.lines() {
        if let Some((n, id)) = parse_session_line(line) {
            if n == name {
                return Some((id, n));
            }
        }
    }
    None
}

/// Per-pane introspection for a session: window, pane, and the inner command.
#[cfg(feature = "tmux")]
fn query_panes(session_id: &str) -> Vec<TmuxPaneInfo> {
    let format = "\
#{window_index}\t\
#{window_name}\t\
#{pane_id}\t\
#{pane_current_command}\t\
#{pane_current_path}\t\
#{pane_pid}";
    let Some(text) = tmux_output(&["list-panes", "-t", session_id, "-s", "-F", format]) else {
        return Vec::new();
    };
    text.lines().filter_map(parse_pane_line).collect()
}

/// Read one full sample ("read"): correlate by PID, fall back to the requested
/// name, then list panes. Returns `None` if the session can't be found.
#[cfg(feature = "tmux")]
pub fn sample_once(child_pid: u32, fallback_name: Option<&str>) -> Option<TmuxSample> {
    let (session_id, session_name) = query_session_for_pid(child_pid)
        .or_else(|| fallback_name.and_then(query_session_by_name))?;
    let panes = query_panes(&session_id);
    Some(TmuxSample {
        session_id: Some(session_id),
        session_name: Some(session_name),
        panes,
    })
}

/// Owner of the sampler thread. Started by `ChildSession` when a local tmux
/// target is active; stopped on child exit so the cached last sample can be
/// snapshotted. Modelled on `ResizeForwarder`.
///
/// Under the `tmux` feature this spawns the polling thread; without it, [`start`]
/// is a no-op stub (no thread, no subprocess) and the slot stays empty.
///
/// [`start`]: TmuxSampler::start
pub struct TmuxSampler {
    /// `Some` only when a real thread was spawned.
    child_alive: Option<Arc<AtomicBool>>,
    handle: Option<JoinHandle<()>>,
}

impl TmuxSampler {
    /// Start the polling sampler against the local tmux server.
    #[cfg(feature = "tmux")]
    pub fn start(
        child_pid: u32,
        target: TmuxMode,
        child_alive: Arc<AtomicBool>,
        state: TmuxState,
    ) -> Self {
        let alive = child_alive.clone();
        let handle = thread::spawn(move || {
            let fallback = target.session().to_string();
            let ticks = (SAMPLE_INTERVAL.as_millis() / POLL_TICK.as_millis()) as u32;
            while alive.load(Ordering::Relaxed) {
                if let Some(sample) = sample_once(child_pid, Some(&fallback)) {
                    state.set(sample);
                }
                for _ in 0..ticks {
                    if !alive.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(POLL_TICK);
                }
            }
        });
        Self {
            child_alive: Some(child_alive),
            handle: Some(handle),
        }
    }

    /// Feature-off stub: no thread, no subprocess. Keeps the slot machinery and
    /// call sites identical so `ChildSession` is feature-agnostic.
    #[cfg(not(feature = "tmux"))]
    #[allow(unused_variables)]
    pub fn start(
        child_pid: u32,
        target: TmuxMode,
        child_alive: Arc<AtomicBool>,
        state: TmuxState,
    ) -> Self {
        Self {
            child_alive: None,
            handle: None,
        }
    }

    pub fn stop(self) {
        if let (Some(alive), Some(handle)) = (self.child_alive, self.handle) {
            alive.store(false, Ordering::Relaxed);
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_client_line_fields() {
        let (pid, sid, name) = parse_client_line("1234\t$3\tdev").unwrap();
        assert_eq!(pid, 1234);
        assert_eq!(sid, "$3");
        assert_eq!(name, "dev");
    }

    #[test]
    fn parse_client_line_rejects_non_pid() {
        assert!(parse_client_line("notapid\t$3\tdev").is_none());
    }

    #[test]
    fn find_session_for_pid_matches() {
        let lines = ["1\t$1\ta", "4242\t$9\tmine", "7\t$2\tc"];
        assert_eq!(
            find_session_for_pid(lines.into_iter(), 4242),
            Some(("$9".into(), "mine".into()))
        );
    }

    #[test]
    fn find_session_for_pid_miss_returns_none() {
        assert!(find_session_for_pid(["1\t$1\ta"].into_iter(), 999).is_none());
    }

    #[test]
    fn parse_session_line_fields() {
        let (name, id) = parse_session_line("dev\t$3").unwrap();
        assert_eq!(name, "dev");
        assert_eq!(id, "$3");
    }

    #[test]
    fn parse_pane_line_all_fields() {
        let info = parse_pane_line("1\tvim\t%0\tvim\t/home/x\t100").unwrap();
        assert_eq!(info.window_index, "1");
        assert_eq!(info.window_name, "vim");
        assert_eq!(info.pane_id, "%0");
        assert_eq!(info.current_command, "vim");
        assert_eq!(info.current_path, "/home/x");
        assert_eq!(info.pane_pid, Some(100));
    }

    #[test]
    fn parse_pane_line_missing_pid_still_parses() {
        let info = parse_pane_line("1\tbash\t%0\tbash\t/x").unwrap();
        assert_eq!(info.pane_pid, None);
    }

    #[test]
    fn parse_pane_line_rejects_too_few_fields() {
        assert!(parse_pane_line("1\tvim").is_none());
    }

    #[test]
    fn tmux_state_set_then_snapshot_roundtrips() {
        let state = TmuxState::new();
        assert!(state.snapshot().is_none());
        let sample = TmuxSample {
            session_id: Some("$3".into()),
            session_name: Some("dev".into()),
            panes: vec![TmuxPaneInfo {
                current_command: "vim".into(),
                ..Default::default()
            }],
        };
        state.set(sample.clone());
        assert_eq!(state.snapshot(), Some(sample));
    }
}
