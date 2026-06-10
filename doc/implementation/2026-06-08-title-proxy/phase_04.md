# title-proxy Implementation Plan

**Goal:** Forward configured process signals to the child process group and propagate terminal resize events to the child PTY.

**Architecture:** Signal registration and forwarding live in `src/signals.rs` as an imperative shell. The proxy loop owns the current child process group and an `Arc<pty_process::blocking::Pty>`; resize and signal threads communicate through shared state rather than embedding signal-specific logic in relay code.

**Tech Stack:** Rust 2021, `signal-hook::iterator::Signals`, `nix::sys::signal`, `pty-process::blocking::Pty::resize` which wraps the PTY resize ioctl.

**Scope:** 6 phases from original design; this file implements phase 4.

**Codebase verified:** 2026-06-08. Phase 2 creates `src/proxy.rs` and `src/terminal.rs`; `src/signals.rs` does not exist before this phase.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### title-proxy.AC5: Signal forwarding
- **title-proxy.AC5.1 Success:** SIGTERM to proxy is forwarded to child process group
- **title-proxy.AC5.2 Success:** SIGHUP is forwarded to child process group by default
- **title-proxy.AC5.3 Success:** Custom signal list in config replaces defaults
- **title-proxy.AC5.4 Edge:** SIGWINCH results in terminal resize propagation to child via `ioctl(TIOCSWINSZ)`

---

<!-- START_SUBCOMPONENT_A (tasks 1-4) -->
<!-- START_TASK_1 -->
### Task 1: Add signal parsing and forwarding module

**Verifies:** title-proxy.AC5.2, title-proxy.AC5.3

**Files:**
- Create: `src/signals.rs`

**Implementation:**

Create `src/signals.rs` as an imperative shell module with the FCIS pattern comment. Implement:
- `parse_signal_name(name: &str) -> eyre::Result<nix::sys::signal::Signal>` supporting names with or without `SIG` prefix, at minimum `TERM`, `HUP`, `INT`, `QUIT`, and `USR1`.
- `parse_forward_signals(names: &[String]) -> eyre::Result<Vec<Signal>>`.
- `forward_to_process_group(child_pid: u32, signal: Signal) -> nix::Result<()>` using the child process group. Prefer `killpg(Pid::from_raw(child_pid as i32), signal)` if `pty-process` makes the child session leader as documented.

**Testing:**

Add unit tests in `src/signals.rs` for signal-name parsing:
- `SIGTERM` and `TERM` parse to `Signal::SIGTERM`.
- `SIGHUP` is accepted.
- Unknown names return an error.
- A custom list replaces defaults by using `Config` from `src/config.rs` rather than appending.

**Verification:**

Run: `cargo test signals::`

Expected: Signal parsing tests pass.

**Commit:** `jj commit -m "Add signal forwarding helpers" src/signals.rs`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Register forwarded signal handler thread

**Verifies:** title-proxy.AC5.1, title-proxy.AC5.2, title-proxy.AC5.3

**Files:**
- Modify: `src/main.rs`
- Modify: `src/proxy.rs`
- Modify: `src/signals.rs`

**Implementation:**

Add `mod signals;` in `src/main.rs`.

In `src/proxy.rs`, after spawning the child and before waiting on it:
- Parse `config.forward_signals`.
- Create `signal_hook::iterator::Signals` for the configured signal numbers.
- Spawn a dedicated thread that iterates received signals and forwards each one to the child process group.
- Use an `Arc<AtomicBool>` shutdown flag so the thread can exit after the child session ends.
- Do not forward SIGWINCH in this task; it is handled separately in Task 3.

If a signal cannot be forwarded because the child already exited, treat that as non-fatal. Other forwarding errors should be logged to stderr or surfaced through a channel; do not panic inside the signal thread.

**Testing:**

Add an integration test in `tests/signals.rs`:
- Run `title-proxy -- sh -c 'trap "echo term; exit 42" TERM; while true; do sleep 1; done'`.
- Send SIGTERM to the proxy process.
- Assert output contains `term` and the proxy exits with the child's resulting code or a documented signal-derived code.

Use condition-based waiting for the child to be ready; do not sleep blindly unless the wait is specifically justified.

**Verification:**

Run: `cargo test --test signals`

Expected: SIGTERM forwarding test passes.

**Commit:** `jj commit -m "Forward configured signals to child group" src/main.rs src/proxy.rs src/signals.rs tests/signals.rs`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Propagate terminal resize to PTY

**Verifies:** title-proxy.AC5.4

**Files:**
- Modify: `src/proxy.rs`

**Implementation:**

In `src/proxy.rs`, add SIGWINCH handling:
- Convert the PTY master to `Arc<pty_process::blocking::Pty>` before spawning relay and resize threads. `Pty` is `Send + Sync`, implements `Read`/`Write` for `&Pty`, and exposes `resize(&self)`, so the relay threads can borrow `&Pty` while the resize thread calls `resize` on another `Arc` clone.
- Register `signal_hook::iterator::Signals` for `SIGWINCH` in a dedicated resize thread.
- On each SIGWINCH, call `terminal::terminal_size(std::io::stdout())`.
- Call `pty.resize(pty_process::Size::new(rows, cols))` on the shared PTY master. `pty-process` documents this as changing the terminal size associated with the PTY; this satisfies the design's `ioctl(TIOCSWINSZ)` requirement through the crate API.
- Treat resize failures after child exit as non-fatal; surface other errors with context.
- Stop the thread when the child session ends.

**Testing:**

Do not create an automated resize integration test in this phase. Terminal resize requires a controllable outer PTY and is tracked as human verification in `test-requirements.md`. Add unit coverage for any pure conversion helper that maps `winsize` rows/columns into `pty_process::Size`.

**Verification:**

Run: `cargo test`

Expected: Automated tests pass.

Manual run: `cargo run -- -- sh -c 'stty size; sleep 5; stty size'`

Expected: Resizing the outer terminal during the sleep changes the second `stty size` output inside the child.

**Commit:** `jj commit -m "Propagate terminal resize to child PTY" src/proxy.rs`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Verify phase 4 behavior

**Files:**
- No file changes unless verification reveals defects

**Verification:**

Run: `cargo test`

Expected: All automated tests pass.

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: Clippy passes.

Manual run: `cargo run -- -- sh -c 'trap "echo hup; exit 44" HUP; while true; do sleep 1; done'`, then send SIGHUP to the proxy.

Expected: Child receives SIGHUP and exits through the trap.

**Commit:** Commit any fixes with a message describing the corrected behavior.
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_A -->
