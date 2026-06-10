# title-proxy Implementation Plan

**Goal:** Observe OSC 0/1/2 terminal-title sequences in the PTY→stdout stream without changing any relayed bytes.

**Architecture:** The OSC parser is a functional core module in `src/osc.rs` with deterministic byte-at-a-time state transitions. The PTY→stdout relay remains the imperative shell and passes bytes through unchanged while feeding copies of those bytes into the parser.

**Tech Stack:** Rust 2021, pure streaming parser, `std::sync::{Arc, Mutex}` for shared title state between relay thread and prompt logic.

**Scope:** 6 phases from original design; this file implements phase 3.

**Codebase verified:** 2026-06-08. Phase 2 creates `src/proxy.rs` and relay helpers; `src/osc.rs` does not exist before this phase.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### title-proxy.AC2: OSC title observation
- **title-proxy.AC2.1 Success:** Child emits `OSC 2;my-title BEL` → proxy captures title "my-title" in memory
- **title-proxy.AC2.2 Success:** Child emits `OSC 0;title ESC \` (ST terminator) → proxy captures title "title"
- **title-proxy.AC2.3 Success:** Title sequences pass through to the outer terminal unchanged
- **title-proxy.AC2.4 Edge:** Title longer than 200 characters is captured correctly
- **title-proxy.AC2.5 Edge:** OSC sequence split across multiple read buffers is parsed correctly

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Add OSC title parser core

**Verifies:** title-proxy.AC2.1, title-proxy.AC2.2, title-proxy.AC2.4, title-proxy.AC2.5

**Files:**
- Create: `src/osc.rs`

**Implementation:**

Create `src/osc.rs` as a functional core module with the FCIS pattern comment. Implement:
- `TitleObserver` with states `Ground`, `Esc`, `OscParam`, and `OscEsc`.
- `TitleObserver::new()`.
- `TitleObserver::advance(byte: u8) -> Option<String>` returning `Some(title)` only when a complete OSC 0/1/2 title sequence terminates.
- `TitleObserver::current_title(&self) -> Option<&str>`.
- Accumulation for OSC payload bytes until BEL (`0x07`) or ST (`ESC \`).

Parser requirements:
- Recognize `ESC ] 0;title BEL`, `ESC ] 1;title BEL`, and `ESC ] 2;title BEL`.
- Recognize the same sequences terminated by ST (`ESC \`).
- Preserve state across multiple calls.
- Capture titles longer than 200 characters; use a reasonable cap such as 4096 bytes to prevent unbounded memory growth and ignore over-cap sequences instead of panicking.
- Return to `Ground` after malformed or completed OSC sequences.

**Testing:**

Add unit tests in `src/osc.rs` for:
- OSC 2 with BEL captures `my-title`.
- OSC 0 with ST captures `title`.
- OSC 1 captures icon title.
- Non-title OSC does not change the current title.
- 250-character title is captured exactly.
- Byte-at-a-time parsing across split boundaries captures the title.
- Malformed/over-cap sequences do not panic and leave parser usable.

**Verification:**

Run: `cargo test osc::`

Expected: OSC parser tests pass.

**Commit:** `jj commit -m "Add OSC title parser" src/osc.rs`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Integrate OSC observer into PTY output relay

**Verifies:** title-proxy.AC2.3

**Files:**
- Modify: `src/main.rs`
- Modify: `src/proxy.rs`

**Implementation:**

Add `mod osc;` in `src/main.rs`.

Update the PTY→stdout relay path in `src/proxy.rs`:
- Create shared title state before spawning relay threads: `Arc<Mutex<Option<String>>>`.
- In the PTY→stdout thread, feed every byte read from the PTY into a local `TitleObserver` before writing the original buffer to stdout.
- When `advance` returns a title, update the shared title state.
- Always write the original buffer to stdout unchanged. Do not filter or rewrite OSC bytes.
- Return the captured title state from the child-session runner so Phase 5 can use it for optional re-emit.

**Testing:**

Add a unit test for a helper such as `observe_titles_in_buffer(observer, title_state, bytes)` if that helper exists. If no helper is needed, rely on `src/osc.rs` parser tests and the integration test in Task 3.

**Verification:**

Run: `cargo test`

Expected: All tests pass.

**Commit:** `jj commit -m "Observe titles during PTY output relay" src/main.rs src/proxy.rs`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Add PTY title pass-through integration test

**Verifies:** title-proxy.AC2.1, title-proxy.AC2.3

**Files:**
- Modify: `tests/relay.rs`

**Testing:**

Extend `tests/relay.rs` with an integration test that runs:

```bash
sh -c 'printf "\033]2;my-title\007visible"'
```

Assert stdout contains the exact OSC bytes followed by `visible`. This verifies pass-through behavior at the process boundary. Captured in-memory title is primarily verified by unit tests and used by Phase 5 re-emit behavior.

**Verification:**

Run: `cargo test --test relay`

Expected: Title sequence pass-through test passes.

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: Clippy passes.

**Commit:** `jj commit -m "Test OSC title pass-through" tests/relay.rs`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->
