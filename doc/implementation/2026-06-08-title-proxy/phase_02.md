# title-proxy Implementation Plan

**Goal:** Spawn a configured child process on a PTY and relay bytes between the user's terminal and that child.

**Architecture:** This phase introduces the imperative runtime shell: terminal raw-mode management in `src/terminal.rs` and PTY process orchestration in `src/proxy.rs`. Relay loops remain blocking and synchronous with `std::thread`, while configuration and exit-status formatting stay separate from I/O-heavy code.

**Tech Stack:** Rust 2021, `pty-process` blocking API, `nix` termios, `std::thread`, `std::io`.

**Scope:** 6 phases from original design; this file implements phase 2.

**Codebase verified:** 2026-06-08. Phase 1 creates `Cargo.toml`, `src/main.rs`, and `src/config.rs`; no PTY, terminal, OSC, or signal modules exist before this phase.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### title-proxy.AC1: PTY relay
- **title-proxy.AC1.1 Success:** Spawning `title-proxy -- bash` gives an interactive shell with full I/O pass-through
- **title-proxy.AC1.2 Success:** Spawning `title-proxy -- ssh user@host` gives an interactive SSH session
- **title-proxy.AC1.3 Edge:** EINTR during relay reads is retried, not treated as EOF

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Add terminal raw-mode guard

**Verifies:** None

**Files:**
- Create: `src/terminal.rs`

**Implementation:**

Create `src/terminal.rs` as an imperative shell module with the FCIS pattern comment. It must define:
- `RawTerminalGuard` that stores the original stdin `Termios`.
- `RawTerminalGuard::enable(stdin: impl AsFd) -> eyre::Result<Self>` using `tcgetattr`, `cfmakeraw`, and `tcsetattr`.
- `Drop` for `RawTerminalGuard` that restores original termios with `TCSANOW`.
- `terminal_size(stdout: impl AsFd) -> eyre::Result<pty_process::Size>` that reads rows and columns using `ioctl(TIOCGWINSZ)` or returns an actionable error.

The drop implementation must not panic. If restore fails in `Drop`, ignore the error because there is no reliable recovery path during unwinding.

**Testing:**

No unit tests for raw terminal syscalls in this task. Behavior is verified by integration/manual PTY checks in later tasks.

**Verification:**

Run: `cargo build`

Expected: Build succeeds after Task 3 wires the module.

**Commit:** `jj commit -m "Add terminal raw mode guard" src/terminal.rs`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Add blocking relay helpers

**Verifies:** title-proxy.AC1.3

**Files:**
- Create: `src/proxy.rs`

**Implementation:**

Create `src/proxy.rs` as an imperative shell module with the FCIS pattern comment. Add small relay helpers before the full proxy loop:
- `copy_retrying_eintr(reader, writer) -> io::Result<u64>` reads into an 8 KiB buffer, retries `ErrorKind::Interrupted`, writes all bytes, flushes after each write, and returns bytes copied on EOF. Use this for PTY→stdout.
- `copy_stdin_to_pty_until_cancelled(stdin_fd, pty_writer, child_alive)` uses `nix::poll::poll` with a short timeout, retries `ErrorKind::Interrupted`, checks `child_alive: Arc<AtomicBool>` before every read and before every write, and exits without consuming prompt bytes once the child session ends.
- `spawn_stdin_to_pty(stdin_fd, pty_writer, child_alive)` starts a thread using `copy_stdin_to_pty_until_cancelled`.
- `spawn_pty_to_stdout(pty_reader, stdout)` starts a thread using `copy_retrying_eintr`.

Do not swallow non-EINTR errors. Return them to the joiner so the main proxy loop can fail with context.

**Testing:**

Add unit tests in `src/proxy.rs` using a custom `Read` implementation that returns `Interrupted` before yielding data. Verify the copied output contains all data and EOF is handled. Add a separate test for the stdin relay cancellation check by setting `child_alive` false before data is read and verifying no bytes are written to the PTY writer.

**Verification:**

Run: `cargo test copy_retrying_eintr`

Expected: EINTR retry test passes.

**Commit:** `jj commit -m "Add blocking relay helpers" src/proxy.rs`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Spawn child on PTY and wire main runtime

**Verifies:** title-proxy.AC1.1, title-proxy.AC1.2

**Files:**
- Modify: `src/main.rs`
- Modify: `src/proxy.rs`

**Implementation:**

Add `pub fn run(config: Config) -> eyre::Result<i32>` in `src/proxy.rs`:
- Resolve `config.command` into program plus args; return a clear error for an empty command.
- Open a blocking PTY with `pty_process::blocking::open()`.
- Query current terminal size with `terminal::terminal_size(std::io::stdout())` and resize the PTY before spawning the child.
- Spawn the child with `pty_process::blocking::Command::new(program).args(args).spawn(pts)`.
- Drop the slave side in the parent after spawning.
- Enable raw mode with `RawTerminalGuard` before starting relay threads.
- Create `child_alive = Arc<AtomicBool>::new(true)` for this child session.
- Start stdin→PTY and PTY→stdout relay threads.
- Wait for the child process.
- Immediately set `child_alive` false after `child.wait()` returns so the stdin relay stops polling before the retry prompt reads from stdin in Phase 5.
- Join the PTY→stdout relay thread and propagate relay errors except expected EOF caused by child exit.
- Join the stdin→PTY relay thread only after cancellation has been signaled and it has returned. It must not remain blocked on a plain stdin `read()`.
- Return the child exit code, or `128 + signal` for signal termination on Unix.

Update `src/main.rs` to call `proxy::run(config)` and exit with the returned code via `std::process::exit(code)`.

**Testing:**

Add an integration test file `tests/relay.rs` that runs the built binary with a non-interactive command such as `sh -c 'printf hello'` and verifies stdout contains `hello`. This does not prove a full interactive shell, but it gives automated coverage for PTY spawn and PTY→stdout pass-through.

Manual verification is required for full interactive AC1.1 and AC1.2.

**Verification:**

Run: `cargo test --test relay`

Expected: Non-interactive PTY relay test passes.

Run: `cargo build`

Expected: Build succeeds.

Manual run: `cargo run -- -- bash`

Expected: An interactive shell opens, accepts input, prints output, and exits when `exit` is typed.

Manual run: `cargo run -- -- ssh user@host`

Expected: SSH session behaves like direct `ssh user@host`; if no host is available, record this as human verification pending.

**Commit:** `jj commit -m "Run child process on PTY" src/main.rs src/proxy.rs tests/relay.rs`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_TASK_4 -->
### Task 4: Verify phase 2 relay behavior

**Files:**
- No file changes unless verification reveals defects

**Verification:**

Run: `cargo test`

Expected: All tests pass.

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: Clippy passes.

Run: `cargo run -- -- bash`

Expected: Interactive shell works with full pass-through.

Run: `cargo run -- -- ssh user@host`

Expected: Interactive SSH works with full pass-through, using a real reachable host.

**Commit:** Commit any fixes with a message describing the corrected behavior.
<!-- END_TASK_4 -->
