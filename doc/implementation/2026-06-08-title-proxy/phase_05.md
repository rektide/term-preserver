# title-proxy Implementation Plan

**Goal:** After child death, preserve screen content, append a status prompt, and let the user retry or quit.

**Architecture:** The proxy runtime becomes a top-level loop that runs one child session at a time. Prompt formatting is pure and testable, while raw keystroke reads, stdout writes, child respawn, and optional title re-emit stay in the imperative proxy shell.

**Tech Stack:** Rust 2021, blocking stdin reads, PTY session loop, OSC title state from Phase 3.

**Scope:** 6 phases from original design; this file implements phase 5.

**Codebase verified:** 2026-06-08. Phase 3 exposes captured title state from `src/proxy.rs`; no rerun prompt exists before this phase.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### title-proxy.AC3: Re-run prompt
- **title-proxy.AC3.1 Success:** Child exits → status line appended below screen content with exit code and "space=retry q=quit"
- **title-proxy.AC3.2 Success:** Pressing spacebar re-spawns the child command
- **title-proxy.AC3.3 Success:** Pressing `q` restores terminal and exits cleanly
- **title-proxy.AC3.4 Success:** Pressing Ctrl+C restores terminal and exits with code 130
- **title-proxy.AC3.5 Edge:** Screen content from child session is preserved when prompt appears

### title-proxy.AC4: Title re-emit on death
- **title-proxy.AC4.1 Success:** When `reemit_title` is true, proxy re-emits `OSC 2;<captured-title> BEL` to stdout when child dies
- **title-proxy.AC4.2 Success:** When `reemit_title` is false (default), no title sequence is emitted on child death

---

<!-- START_SUBCOMPONENT_A (tasks 1-4) -->
<!-- START_TASK_1 -->
### Task 1: Add prompt formatting core

**Verifies:** title-proxy.AC3.1

**Files:**
- Create: `src/prompt.rs`
- Modify: `src/main.rs`

**Implementation:**

Create `src/prompt.rs` as a functional core module with the FCIS pattern comment, and add `mod prompt;` in `src/main.rs`. Implement:
- `format_status_prompt(exit_code: i32) -> String` returning `\r\n[title-proxy] child exited with code {code}; space=retry q=quit`.
- `title_reemit_sequence(title: &str) -> Vec<u8>` returning `OSC 2;<title> BEL`.

**Testing:**

Add unit tests for:
- Prompt includes exit code and exact text `space=retry q=quit`.
- Prompt starts with `\r\n` so it appends below existing screen content.
- Title re-emit sequence is exactly `b"\x1b]2;<title>\x07"`.

**Verification:**

Run: `cargo test prompt::`

Expected: Prompt formatting tests pass.

**Commit:** `jj commit -m "Add child-exit prompt formatting" src/prompt.rs src/main.rs`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Implement retry prompt loop

**Verifies:** title-proxy.AC3.1, title-proxy.AC3.2, title-proxy.AC3.3, title-proxy.AC3.4, title-proxy.AC3.5

**Files:**
- Modify: `src/proxy.rs`
- Modify: `tests/relay.rs`
- Create: `tests/prompt.rs`

**Implementation:**

Change `proxy::run(config)` from a single child session to a loop:
- Enable raw terminal mode once before the loop and hold `RawTerminalGuard` until final exit.
- Run one child session with the existing PTY spawn and relay logic.
- After child exit and relay thread cleanup, write the status prompt to stdout without clearing the screen.
- Ensure the stdin→PTY relay has observed `child_alive = false` and returned before reading the prompt byte. This prevents the old relay thread from consuming the prompt key.
- Read one byte from stdin in raw mode.
- If byte is `0x20`, loop and spawn the same configured command again.
- If byte is `b'q'`, drop `RawTerminalGuard` and return exit code 0.
- If byte is `0x03`, drop `RawTerminalGuard` and return exit code 130.
- For any other byte, keep waiting for a valid prompt choice.

Do not use alternate screen mode and do not clear terminal content. The prompt must be appended below whatever the child left on screen.

**Testing:**

Use the `expectrl` dev-dependency added in Phase 1 for PTY automation in `tests/prompt.rs`. Spawn the built `title-proxy` binary under `expectrl`, wait for marker output with condition-based expectations, send prompt keystrokes with `send(" ")`, `send("q")`, or Ctrl+C, and only then wait for process exit.

Add integration tests in `tests/prompt.rs`:
- Child command prints a marker and exits.
- Test sends space, observes marker appears a second time.
- Test sends `q`, observes clean exit.
- Test sends Ctrl+C at prompt, observes exit code 130.

Update every existing process-level test in `tests/relay.rs` that runs a child command which exits. After asserting the expected child output, drive the retry prompt with `q` before waiting for the proxy process to exit. Do not leave any post-Phase-5 integration test that only asserts child output and then waits; it will hang at the prompt.

**Verification:**

Run: `cargo test`

Expected: Automated tests pass.

Manual run: `cargo run -- -- sh -c 'echo run; exit 7'`

Expected: Prompt appears after `run`; pressing space prints `run` again; pressing `q` exits cleanly.

Manual run: `cargo run -- -- sh -c 'echo run; exit 7'`, then press Ctrl+C at prompt.

Expected: Terminal restores and process exits 130.

**Commit:** `jj commit -m "Add retry prompt loop" src/proxy.rs tests/relay.rs tests/prompt.rs`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Implement optional title re-emit

**Verifies:** title-proxy.AC4.1, title-proxy.AC4.2

**Files:**
- Modify: `src/proxy.rs`
- Modify: `tests/relay.rs`

**Implementation:**

After one child session exits and before writing the status prompt:
- If `config.reemit_title` is true and the captured title state is `Some(title)`, write `OSC 2;<title> BEL` to stdout and flush.
- If `config.reemit_title` is false, write no title sequence.
- Add a code comment at this branch stating that title re-emit is experimental and may be removed if terminals preserve the latest title reliably without it.

**Testing:**

Add integration tests that run a child command emitting `OSC 2;captured BEL` and exiting:
- With default config, assert stdout contains only the child's original OSC title sequence and no second title sequence after child exit.
- With `--reemit-title`, assert stdout contains the child's original OSC sequence and a second `OSC 2;captured BEL` before the prompt.

**Verification:**

Run: `cargo test --test relay`

Expected: Title re-emit tests pass.

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: Clippy passes.

**Commit:** `jj commit -m "Optionally re-emit captured title" src/proxy.rs tests/relay.rs`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Verify phase 5 prompt behavior

**Files:**
- No file changes unless verification reveals defects

**Verification:**

Run: `cargo test`

Expected: All automated tests pass.

Run: `cargo run -- -- sh -c 'printf "before\\n"; exit 7'`

Expected: `before` remains visible, prompt appears below it with exit code 7 and `space=retry q=quit`.

Run: `cargo run -- --reemit-title -- sh -c 'printf "\\033]2;phase5\\007"'`

Expected: Captured title is re-emitted after child exit before prompt.

**Commit:** Commit any fixes with a message describing the corrected behavior.
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_A -->
