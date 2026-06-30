//! Integration tests for the tmux sampler against a real local tmux server.
//! Gated on `tmux` being installed; both tests early-return (pass) otherwise.
//! Compiled only with the `tmux` feature, which provides the sourcing API.

#![cfg(feature = "tmux")]

use std::process::Command as StdCommand;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use term_preserver::launch::TmuxMode;
use term_preserver::tmux::{sample_once, TmuxSampler, TmuxState};

fn tmux_available() -> bool {
    StdCommand::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A unique-ish session name so parallel tests in the same binary don't collide.
fn session_name(tag: &str) -> String {
    format!("tp-{tag}-{}", std::process::id())
}

fn create_detached_session(name: &str) {
    StdCommand::new("tmux")
        .args(["new-session", "-d", "-s", name, "sleep", "300"])
        .status()
        .expect("spawning tmux session");
}

fn kill_session(name: &str) {
    let _ = StdCommand::new("tmux")
        .args(["kill-session", "-t", name])
        .status();
}

#[test]
fn sample_once_reads_local_session_by_name() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let name = session_name("direct");
    create_detached_session(&name);

    // No client matches a bogus pid, so this exercises the name fallback path.
    let sample = sample_once(u32::MAX, Some(&name));
    kill_session(&name);

    let sample = sample.expect("expected a sample for a known local session");
    assert_eq!(sample.session_name.as_deref(), Some(name.as_str()));
    assert!(
        sample.session_id.is_some(),
        "session id should be populated: {sample:?}"
    );
    assert!(
        !sample.panes.is_empty(),
        "expected at least one pane: {sample:?}"
    );
}

#[test]
fn sampler_thread_records_session_into_state() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let name = session_name("sampler");
    create_detached_session(&name);

    let state = TmuxState::new();
    let alive = Arc::new(AtomicBool::new(true));
    let sampler = TmuxSampler::start(
        u32::MAX,
        TmuxMode::CreateOrAttach(name.clone()),
        alive.clone(),
        state.clone(),
    );

    // The sampler takes an immediate first sample; give it a moment to land.
    thread::sleep(Duration::from_millis(500));
    alive.store(false, Ordering::Relaxed);
    sampler.stop();
    kill_session(&name);

    let snap = state
        .snapshot()
        .expect("sampler should have recorded a sample into shared state");
    assert_eq!(snap.session_name.as_deref(), Some(name.as_str()));
}
