# title-proxy Implementation Plan

**Goal:** Harden the proxy for end-to-end use with shells, SSH, title-setting programs, signal exits, terminal restoration, and edge cases.

**Architecture:** This phase closes behavior gaps rather than adding major modules. It adds RAII guards and panic restoration around imperative terminal code, improves error context, and expands integration/human verification coverage for the full proxy workflow.

**Tech Stack:** Rust 2021, `eyre` context, RAII terminal restoration, integration tests, manual PTY verification.

**Scope:** 6 phases from original design; this file implements phase 6.

**Codebase verified:** 2026-06-08. Previous phases create all runtime modules; this phase modifies them for restoration, error handling, and end-to-end verification.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### title-proxy.AC6: Configuration
- **title-proxy.AC6.1 Success:** CLI args set command, forward_signals, reemit_title
- **title-proxy.AC6.2 Success:** Config file is loaded and merged with correct precedence (CLI > env > file > defaults)
- **title-proxy.AC6.3 Success:** Default command is `$SHELL`

### title-proxy.AC7: Terminal restoration
- **title-proxy.AC7.1 Success:** Terminal is restored to cooked mode on normal exit
- **title-proxy.AC7.2 Success:** Terminal is restored to cooked mode on signal-induced exit
- **title-proxy.AC7.3 Success:** Terminal is restored to cooked mode on panic

It also performs final end-to-end verification for AC1 through AC5.

---

<!-- START_SUBCOMPONENT_A (tasks 1-4) -->
<!-- START_TASK_1 -->
### Task 1: Complete configuration coverage

**Verifies:** title-proxy.AC6.1, title-proxy.AC6.2, title-proxy.AC6.3

**Files:**
- Modify: `src/config.rs`
- Modify: `tests/config.rs`

**Implementation:**

Move config tests out of `src/config.rs` into `tests/config.rs` if they require process environment mutation or file-system setup that is cleaner as integration tests. Keep pure unit tests in `src/config.rs` for default helper functions.

Ensure the config loader supports:
- `--forward-signals SIGTERM,SIGHUP` or repeated `--forward-signals SIGTERM --forward-signals SIGHUP`; choose one syntax and make help text clear.
- `--reemit-title` setting `Config.reemit_title = true`.
- trailing command after `--` replacing the default command.
- config file path through `--config`.
- `TITLE_PROXY_` environment variables with documented separators.

**Testing:**

Tests must verify:
- CLI command overrides default command.
- CLI `--reemit-title` overrides config file false value.
- Env overrides config file.
- CLI overrides env.
- Default command uses `$SHELL` when set.

Every test that mutates `TITLE_PROXY_` variables or `$SHELL` must use `serial_test::serial`, save the previous value, and restore it before returning. These environment variables are process-global, so Phase 6 tests must not rely on parallel test isolation.

**Verification:**

Run: `cargo test config`

Expected: Configuration tests pass.

**Commit:** `jj commit -m "Complete configuration precedence tests" src/config.rs tests/config.rs`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Harden terminal restoration

**Verifies:** title-proxy.AC7.1, title-proxy.AC7.2, title-proxy.AC7.3

**Files:**
- Modify: `src/main.rs`
- Modify: `src/proxy.rs`
- Modify: `src/terminal.rs`

**Implementation:**

Ensure terminal restoration is structurally guaranteed:
- `RawTerminalGuard` must be created before the child-session loop and remain in scope until final process exit.
- Normal `q` exit returns through code that drops the guard.
- Ctrl+C at prompt returns code 130 through code that drops the guard.
- Signal-induced shutdown forwards signal to child, waits for session cleanup, then returns through code that drops the guard.
- Install a panic hook in `src/main.rs` only after capturing enough state to restore terminal. The hook should call a minimal terminal restore function if raw mode is active, then delegate to the previous panic hook.

Avoid a global mutable terminal state unless necessary. If necessary, hide it behind a small `terminal` module API and document why panic restoration requires it.

**Testing:**

Automated cooked-mode tests are environment-sensitive. Add tests only if the test harness runs the proxy under a PTY and can query `stty -a` before and after. Otherwise mark AC7.1 through AC7.3 for human verification in `test-requirements.md`.

**Verification:**

Run: `cargo test`

Expected: Automated tests pass.

Manual run: `stty -g`, then `cargo run -- -- sh -c 'exit 0'`, press `q`, then `stty -g`.

Expected: The two `stty -g` outputs match.

Manual run: `cargo run -- -- sh -c 'while true; do sleep 1; done'`, send SIGTERM to the proxy, then run `stty -g`.

Expected: Terminal remains usable and cooked.

**Commit:** `jj commit -m "Guarantee terminal restoration" src/main.rs src/proxy.rs src/terminal.rs`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Add end-to-end smoke tests and error context

**Verifies:** title-proxy.AC1.3 and regression coverage for AC1 through AC5

**Files:**
- Modify: `src/proxy.rs`
- Modify: `src/terminal.rs`
- Modify: `src/signals.rs`
- Modify: `tests/relay.rs`
- Modify: `tests/signals.rs`

**Implementation:**

Add `eyre::WrapErr` context to failure points:
- opening PTY
- resizing PTY
- spawning child command
- enabling raw mode
- spawning signal handlers
- relay thread failures
- prompt stdin read failures

Review EINTR handling for all blocking reads that title-proxy owns. Reads in relay helpers and prompt input must retry `ErrorKind::Interrupted`.

**Testing:**

Add or update smoke tests for:
- non-interactive local command output
- command not found reports useful error
- title sequence pass-through
- prompt output after child exit
- SIGTERM forwarding

Tests should avoid SSH because that depends on external infrastructure. SSH remains manual/human verification. Every smoke test that runs a command which exits after Phase 5 must drive the retry prompt and send `q` or Ctrl+C before waiting for the proxy to exit.

**Verification:**

Run: `cargo test`

Expected: All smoke tests pass.

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: Clippy passes.

**Commit:** `jj commit -m "Harden proxy error handling" src/proxy.rs src/terminal.rs src/signals.rs tests/relay.rs tests/signals.rs`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Complete manual verification checklist

**Files:**
- Create: `doc/implementation/2026-06-08-title-proxy/manual-verification.md`

**Implementation:**

Create `manual-verification.md` with commands and result fields for behavior that cannot be reliably automated in ordinary CI:
- Interactive bash pass-through.
- Interactive SSH pass-through to a real reachable host.
- Terminal resize propagation with `stty size` inside the child.
- Terminal restoration after normal prompt quit.
- Terminal restoration after Ctrl+C at prompt.
- Terminal restoration after SIGTERM to proxy.
- Panic restoration by adding a temporary `TITLE_PROXY_TEST_PANIC_AFTER_RAW=1` environment-gated panic immediately after raw mode is enabled, running the proxy under a PTY, verifying terminal mode restoration, and removing the temporary panic hook trigger before committing production code. The committed code must not include this test-only panic path.

**Verification:**

Run every checklist item locally before final release. Fill in observed result, date, terminal emulator, shell, and platform.

Run: `cargo test`

Expected: Automated tests still pass after documentation changes.

**Commit:** `jj commit -m "Document manual PTY verification" doc/implementation/2026-06-08-title-proxy/manual-verification.md`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_TASK_5 -->
### Task 5: Final phase verification

**Files:**
- No file changes unless verification reveals defects

**Verification:**

Run: `cargo build`

Expected: Build succeeds.

Run: `cargo test`

Expected: All tests pass.

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: Clippy passes.

Run: `cargo run -- --help`

Expected: Help clearly documents config file, env prefix, forwarded signal syntax, `--reemit-title`, and trailing child command.

**Commit:** Commit any fixes with a message describing the corrected behavior.
<!-- END_TASK_5 -->
