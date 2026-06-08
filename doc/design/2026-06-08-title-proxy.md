# title-proxy Design

## Summary

title-proxy sits between your terminal and a child process (like a shell or SSH session), transparently relaying all keystrokes and output through a pseudo-terminal. As bytes flow from the child back to your screen, a lightweight streaming parser watches for OSC title-setting sequences without altering the data stream, keeping the current terminal title in memory. The entire proxy is synchronous — two blocking relay threads handle stdin→PTY and PTY→stdout, with no async runtime — keeping the implementation small and easy to reason about.

When the child process exits, title-proxy preserves whatever was on screen, appends a status line showing the exit code, and offers a simple prompt: press space to re-run the same command, or `q` to quit. This re-run loop makes it useful as a wrapper around flaky SSH connections or any command you want to respawn without retyping. The design borrows its relay thread structure and OSC parsing approach from atuin's pty-proxy crate, but strips it down to just title observation — no screen state tracking, no database, no network.

## Definition of Done

**title-proxy** is a Rust binary that acts as a lightweight PTY proxy between the user's terminal and a child process. It:

1. Spawns a child process (shell or ssh) on a PTY and transparently relays all bytes in both directions
2. Observes OSC 0/1/2 sequences in the PTY→stdout stream, capturing the current title in memory (no disk persistence in MVP)
3. When the child dies, preserves the terminal screen content, moves to the bottom, and prints a prompt showing exit info with options: spacebar to re-run the same command, `q` to quit
4. Optionally re-emits the captured title on child death (configurable, default off — annotated in code as potentially unnecessary, may be removed)
5. Propagates signals to the child process, configurable via figment2 config (which signals to pass through)
6. Uses figment2 for all configuration (command to run, signal passthrough, title re-emit behavior, etc.)
7. Propagates terminal resize (SIGWINCH) to the child PTY
8. No persistence, no network, no async runtime — blocking I/O with std::thread, inspired by atuin's pty-proxy architecture
9. Design docs written to `doc/design/`

## Acceptance Criteria

### title-proxy.AC1: PTY relay
- **title-proxy.AC1.1 Success:** Spawning `title-proxy -- bash` gives an interactive shell with full I/O pass-through
- **title-proxy.AC1.2 Success:** Spawning `title-proxy -- ssh user@host` gives an interactive SSH session
- **title-proxy.AC1.3 Edge:** EINTR during relay reads is retried, not treated as EOF

### title-proxy.AC2: OSC title observation
- **title-proxy.AC2.1 Success:** Child emits `OSC 2;my-title BEL` → proxy captures title "my-title" in memory
- **title-proxy.AC2.2 Success:** Child emits `OSC 0;title ESC \` (ST terminator) → proxy captures title "title"
- **title-proxy.AC2.3 Success:** Title sequences pass through to the outer terminal unchanged
- **title-proxy.AC2.4 Edge:** Title longer than 200 characters is captured correctly
- **title-proxy.AC2.5 Edge:** OSC sequence split across multiple read buffers is parsed correctly

### title-proxy.AC3: Re-run prompt
- **title-proxy.AC3.1 Success:** Child exits → status line appended below screen content with exit code and "space=retry q=quit"
- **title-proxy.AC3.2 Success:** Pressing spacebar re-spawns the child command
- **title-proxy.AC3.3 Success:** Pressing `q` restores terminal and exits cleanly
- **title-proxy.AC3.4 Success:** Pressing Ctrl+C restores terminal and exits with code 130
- **title-proxy.AC3.5 Edge:** Screen content from child session is preserved when prompt appears

### title-proxy.AC4: Title re-emit on death
- **title-proxy.AC4.1 Success:** When `reemit_title` is true, proxy re-emits `OSC 2;<captured-title> BEL` to stdout when child dies
- **title-proxy.AC4.2 Success:** When `reemit_title` is false (default), no title sequence is emitted on child death

### title-proxy.AC5: Signal forwarding
- **title-proxy.AC5.1 Success:** SIGTERM to proxy is forwarded to child process group
- **title-proxy.AC5.2 Success:** SIGHUP is forwarded to child process group by default
- **title-proxy.AC5.3 Success:** Custom signal list in config replaces defaults
- **title-proxy.AC5.4 Edge:** SIGWINCH results in terminal resize propagation to child via `ioctl(TIOCSWINSZ)`

### title-proxy.AC6: Configuration
- **title-proxy.AC6.1 Success:** CLI args set command, forward_signals, reemit_title
- **title-proxy.AC6.2 Success:** Config file is loaded and merged with correct precedence (CLI > env > file > defaults)
- **title-proxy.AC6.3 Success:** Default command is `$SHELL`

### title-proxy.AC7: Terminal restoration
- **title-proxy.AC7.1 Success:** Terminal is restored to cooked mode on normal exit
- **title-proxy.AC7.2 Success:** Terminal is restored to cooked mode on signal-induced exit
- **title-proxy.AC7.3 Success:** Terminal is restored to cooked mode on panic

## Glossary

- **PTY (Pseudo-Terminal)**: A pair of virtual devices (master and slave) that emulates a physical terminal. The proxy owns the master side; the child process runs on the slave side, seeing what looks like a real terminal.
- **OSC (Operating System Command)**: A terminal escape sequence starting with `ESC ]` followed by a numeric code and parameters. OSC codes 0, 1, and 2 set the terminal title or icon name.
- **BEL**: The bell character (`0x07`), one of two standard terminators for an OSC sequence.
- **ST (String Terminator)**: The two-character sequence `ESC \`, the other standard OSC terminator. Both BEL and ST are handled by the parser.
- **Raw mode**: A terminal configuration where the kernel's line discipline is disabled — no line buffering, no echo, no control-character signal generation. Required so keystrokes reach the child process unmodified.
- **Cooked mode**: The default terminal mode with line buffering, echo, and kernel-generated signals (e.g., Ctrl+C → SIGINT). The proxy restores this on exit.
- **Line discipline**: The kernel component that processes terminal I/O. In raw mode it's bypassed; in the PTY, it still translates control characters like Ctrl+C into signals for the child automatically.
- **ioctl**: A Unix system call for device-specific operations. Used here with `TIOCGWINSZ` (get window size) and `TIOCSWINSZ` (set window size) to propagate terminal resizes.
- **EINTR**: The "interrupted system call" error returned when a blocking read or write is interrupted by a signal. The proxy retries these rather than treating them as EOF.
- **Process group**: A collection of related processes. Signals forwarded via `kill(-pid, signal)` target the child's entire process group, ensuring grandchildren (e.g., a remote shell spawned by SSH) also receive the signal.
- **Streaming state machine**: A parser that processes bytes one at a time, maintaining state across multiple read buffers. Essential because an OSC sequence can be split across separate `read()` calls.
- **figment2**: A Rust configuration library that merges values from layered providers (CLI args, environment variables, config files, hardcoded defaults) with defined precedence.
- **clap**: A Rust CLI argument parsing library, used here with derive macros on the config struct.
- **bon**: A Rust crate that generates a builder pattern from struct definitions.
- **signal-hook**: A Rust crate for registering async-signal-safe Unix signal handlers.
- **pty-process**: A Rust crate for creating pseudo-terminals and spawning child processes on them. Chosen over `portable-pty` for its lighter, Linux-focused API.
- **nix**: Rust bindings to POSIX system calls — used here for termios (raw/cooked mode), ioctl (window size), and process signaling.
- **atuin**: A shell history tool whose PTY-proxy crate inspired this project's relay loop structure and streaming OSC parser approach.
- **VT100**: The DEC terminal whose escape sequence set became the basis for modern terminal emulation. The proxy assumes a minimally VT100-compatible terminal.

## Architecture

### Data Flow

```
stdin ──────────► PTY writer
                   │
                   │ (child process on PTY slave)
                   │
PTY reader ──────► stdout (via OSC parser)
                   │
                   ├─ bytes pass through unchanged
                   └─ OSC 0/1/2 titles captured in memory

SIGWINCH ────────► ioctl(TIOCSWINSZ) on PTY master fd

Configured signals (default: SIGTERM, SIGHUP)
              ────► kill(-child_pid, signal) to child process group

child exits ─────► optional re-emit of captured title
                   \r\n + status line appended below screen content
                   "space=retry  q=quit" prompt
                   space → respawn child
                   q → restore terminal, exit
```

### Main Loop

The proxy runs a single top-level loop:

1. Put the real terminal (stdin/stdout) into raw mode via `nix::sys::termios`
2. Open a PTY, spawn the child command
3. Start two relay threads:
   - **stdin→PTY**: blocking `read()` from stdin, `write_all()` to PTY master writer
   - **PTY→stdout**: blocking `read()` from PTY master reader, pass through OSC parser, `write_all()` to stdout
4. Start a SIGWINCH handler thread: reads new terminal size, calls `ioctl(TIOCSWINSZ)` on PTY master fd
5. Register signal handlers for configured forwarded signals; forward to child process group via `kill(-child_pid, signal)`
6. Wait for child to exit (`child.wait()`)
7. Join relay threads
8. Optionally re-emit captured title (if `reemit_title` config is true)
9. Write `\r\n` + status line to stdout (below existing screen content)
10. Read single byte from stdin (still in raw mode):
    - `0x20` (space) → loop to step 2
    - `q` → disable raw mode, exit 0
    - `0x03` (Ctrl+C) → disable raw mode, exit 130

No async runtime. `std::thread` + blocking I/O throughout.

### OSC Parser

A streaming state machine that observes the PTY→stdout byte stream without modifying it. Adapted from atuin's `osc133.rs` architecture:

```
Ground → (ESC) → Esc → (']') → OscParam → (BEL or ESC\) → Ground
```

In `OscParam` state, the parser accumulates bytes. When it sees prefix `0;`, `1;`, or `2;`, it captures the subsequent string as the current title. Both terminators are handled: BEL (`0x07`) and ST (`ESC \`).

Key differences from atuin's parser:
- Matches `0;`, `1;`, `2;` instead of `133;`
- Larger param buffer (titles can be 200+ chars; atuin uses 32 bytes)
- Inline in the relay thread — no separate parser thread needed
- No vt100 shadow parser — we don't track screen state

The parser never modifies the data stream. It only observes and records title strings.

### Terminal Management

The proxy interacts with the terminal directly via `nix` syscalls, no crossterm dependency:

- **Raw mode**: `tcgetattr` + `cfmakeraw` + `tcsetattr` on stdin
- **Terminal size**: `ioctl` with `TIOCGWINSZ` on stdout
- **Resize propagation**: `ioctl` with `TIOCSWINSZ` on PTY master fd
- **Cursor movement / clear line**: raw escape sequences written to stdout

The proxy is deliberately ignorant of terminal capabilities beyond what POSIX provides. It assumes a minimally VT100-compatible terminal.

### Signal Handling

Signals the PTY child receives automatically (no forwarding needed):
- SIGINT, SIGQUIT, SIGTSTP — generated by the kernel's line discipline when the corresponding control character is written to the PTY master
- SIGWINCH — sent by the kernel when `ioctl(TIOCSWINSZ)` is called on the master
- SIGTTIN, SIGTTOU — handled by the kernel for background process group I/O

Signals that need explicit forwarding:
- **SIGHUP** — sent to child process group on proxy shutdown
- **SIGTERM** — forwarded when the proxy receives SIGTERM
- **Any additional signals** configured in `forward_signals`

The proxy uses `signal-hook` crate to register handlers. Forwarded signals are sent to the child's entire process group via `kill(-child_pid, signal)` (negative PID).

### Configuration

```rust
#[derive(clap::Parser, serde::Deserialize, serde::Serialize, bon::Builder)]
struct Config {
    /// Command to run (default: $SHELL)
    #[arg(trailing_var_arg = true)]
    command: Vec<String>,

    /// Signals to forward to child process group
    /// Default: ["SIGTERM", "SIGHUP"]
    forward_signals: Vec<String>,

    /// Re-emit captured title when child dies
    /// Default: false
    reemit_title: bool,
}
```

Config loaded via figment2 with precedence: CLI args > env vars > config file > defaults. Following the project convention: `figment2::providers::Serialized::defaults(CliArgs::parse())`.

### Dependencies

| Crate | Purpose |
|-------|---------|
| `pty-process` (blocking) | PTY management: open, spawn, resize, read/write |
| `nix` | Termios (raw mode), `TIOCGWINSZ`/`TIOCSWINSZ`, process signals |
| `signal-hook` | SIGWINCH detection, configured signal forwarding |
| `figment2` | Configuration loading with layered providers |
| `clap` v4 (derive) | CLI argument parsing |
| `serde` + `serde_json` | Config struct serialization for figment2 |
| `bon` | Builder pattern on config struct |
| `eyre` | Error handling |

## Existing Patterns

Greenfield project — no existing codebase patterns to follow. The design draws from:

- **atuin pty-proxy** (`~/archive/atuinsh/atuin/crates/atuin-pty-proxy/`): PTY setup with `portable-pty`, blocking I/O relay threads, OSC 133 streaming parser, SIGWINCH handling. We adapt the OSC parser pattern and relay loop structure.
- **AGENTS.md conventions**: figment2 for config, clap v4 with derive, `bon` for builder, `eyre` for errors, `jiff` for time if needed.

Key divergence from atuin: we use `pty-process` instead of `portable-pty` (lighter, Linux-focused, concrete types), `nix` instead of `crossterm` (no dependency on terminal abstraction layer), and no vt100 shadow parser (title-only observation).

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Project Scaffolding
**Goal:** Rust project with dependencies, CLI parsing, and config loading

**Components:**
- `Cargo.toml` with all dependencies (pty-process, nix, signal-hook, figment2, clap, serde, bon, eyre)
- `src/main.rs` entry point with clap derive and figment2 config loading
- `src/config.rs` — Config struct with clap/serde/bon derives

**Dependencies:** None (first phase)

**Done when:** `cargo build` succeeds, `cargo clippy` passes, binary prints help text
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: PTY Spawning and Relay
**Goal:** Spawn a child process on a PTY and relay bytes in both directions

**Components:**
- `src/proxy.rs` — main proxy loop: open PTY, spawn child, start relay threads, wait for child
- `src/terminal.rs` — raw mode enable/disable, terminal size query via nix

**Dependencies:** Phase 1

**Done when:** Running `title-proxy -- ssh user@host` establishes an interactive SSH session with full I/O pass-through, `cargo clippy` passes
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: OSC Title Observer
**Goal:** Streaming OSC parser that captures title from the PTY→stdout stream

**Components:**
- `src/osc.rs` — streaming state machine parser for OSC 0/1/2 sequences, observes without modifying the stream

**Dependencies:** Phase 2 (parser integrates into the PTY→stdout relay thread)

**Done when:** Title changes from child process (e.g. shell prompt, ssh) are captured in memory while all bytes pass through unchanged, verified with a test that feeds OSC sequences and checks captured title
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Signal Forwarding and Resize
**Goal:** Forward configured signals to child process group, propagate terminal resize

**Components:**
- `src/signals.rs` — register signal handlers using `signal-hook`, forward to child process group via `kill(-pid, sig)`
- SIGWINCH handler thread in `src/proxy.rs` — read terminal size, call `ioctl(TIOCSWINSZ)` on PTY master

**Dependencies:** Phase 2

**Done when:** SIGTERM to proxy is forwarded to child, terminal resize reaches child process, configured signal list is respected
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Re-run Prompt on Child Death
**Goal:** When child dies, show status line and wait for spacebar (retry) or q (quit)

**Components:**
- Re-run loop in `src/proxy.rs` — join relay threads, write status line, read single keystroke
- Optional title re-emit (if `reemit_title` config is true) — write captured OSC title to stdout

**Dependencies:** Phase 3 (needs captured title for optional re-emit)

**Done when:** Child death shows exit status + "space=retry q=quit" prompt appended below screen content, spacebar re-spawns child, q exits cleanly, optional title re-emit works when enabled
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: Integration and Polish
**Goal:** End-to-end testing, error handling, edge cases

**Components:**
- Error handling for PTY failures, EINTR in reads, child signal propagation
- Clean terminal restoration on all exit paths (panic, signal, normal)
- Verify behavior with ssh, local shells, and programs that set titles

**Dependencies:** Phase 5

**Done when:** Proxy works reliably with ssh connections (including connection loss), local shells, and programs that set/change terminal titles; terminal is always restored to cooked mode on exit
<!-- END_PHASE_6 -->
