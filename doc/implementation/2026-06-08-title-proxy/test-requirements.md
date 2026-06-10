# title-proxy Test Requirements

## Automated Tests

| Acceptance criterion | Test type | Expected test file path |
|---|---:|---|
| title-proxy.AC1.3: EINTR during relay reads is retried, not EOF | unit | `src/proxy.rs` |
| title-proxy.AC2.1: `OSC 2;my-title BEL` captures `my-title` | unit + integration | `src/osc.rs`, `tests/relay.rs` |
| title-proxy.AC2.2: `OSC 0;title ESC \` captures `title` | unit | `src/osc.rs` |
| title-proxy.AC2.3: Title sequences pass through unchanged | integration | `tests/relay.rs` |
| title-proxy.AC2.4: Title longer than 200 chars is captured | unit | `src/osc.rs` |
| title-proxy.AC2.5: Split OSC sequence parses correctly | unit | `src/osc.rs` |
| title-proxy.AC3.1: Child exit appends status line with exit code and `space=retry q=quit` | unit + integration | `src/prompt.rs`, `tests/prompt.rs` |
| title-proxy.AC3.2: Spacebar re-spawns child command | e2e | `tests/prompt.rs` |
| title-proxy.AC3.3: `q` exits cleanly | e2e | `tests/prompt.rs` |
| title-proxy.AC3.4: Ctrl+C at prompt exits 130 | e2e | `tests/prompt.rs` |
| title-proxy.AC3.5: Child screen content is preserved when prompt appears | integration | `tests/prompt.rs` |
| title-proxy.AC4.1: `reemit_title=true` emits `OSC 2;<captured-title> BEL` on child death | integration | `tests/relay.rs` |
| title-proxy.AC4.2: default `reemit_title=false` emits no extra title sequence | integration | `tests/relay.rs` |
| title-proxy.AC5.1: SIGTERM to proxy forwards to child process group | integration | `tests/signals.rs` |
| title-proxy.AC5.2: SIGHUP is forwarded by default | unit + integration | `src/signals.rs`, `tests/signals.rs` |
| title-proxy.AC5.3: Custom signal list replaces defaults | unit + integration | `src/signals.rs`, `tests/config.rs` |
| title-proxy.AC6.1: CLI args set command, `forward_signals`, `reemit_title` | integration | `tests/config.rs` |
| title-proxy.AC6.2: Config precedence is CLI > env > file > defaults | integration | `tests/config.rs` |
| title-proxy.AC6.3: Default command is `$SHELL` | unit + integration | `src/config.rs`, `tests/config.rs` |

Prompt-related integration tests must use `expectrl` to drive the post-child-exit prompt before waiting for process exit. After the child exits, send `q` for normal termination or Ctrl+C for AC3.4; otherwise tests will hang at the retry prompt. Existing relay tests must be updated in Phase 5 to send `q` after the retry prompt appears.

## Human Verification

| Acceptance criterion | Why not normal CI | Concrete verification approach |
|---|---|---|
| title-proxy.AC1.1: `title-proxy -- bash` gives an interactive shell with full I/O pass-through | Full interactive terminal behavior depends on a real terminal emulator, line discipline, shell job control, and human keystrokes. CI smoke tests only prove non-interactive PTY relay. | Run `cargo run -- -- bash`; verify typing commands, Ctrl+C inside child, output, prompt rendering, and `exit` behavior match direct bash. |
| title-proxy.AC1.2: `title-proxy -- ssh user@host` gives an interactive SSH session | Requires reachable SSH infrastructure, credentials, network stability, host key handling, and real TTY negotiation. This is not reliable in ordinary CI. | Run `cargo run -- -- ssh user@host` against a real host; verify login, remote shell I/O, disconnect behavior, and retry prompt after connection loss. |
| title-proxy.AC5.4: SIGWINCH propagates terminal resize to child via PTY resize/ioctl | Reliable automation requires a controllable outer PTY and terminal-size mutation harness. Planning explicitly tracks resize as human verification. | Run `cargo run -- -- sh -c 'stty size; sleep 5; stty size'`; resize the outer terminal during sleep; verify the second size changes inside the child. |
| title-proxy.AC7.1: Terminal restores to cooked mode on normal exit | Cooked/raw mode restoration depends on the controlling terminal. Mark human unless a PTY harness can compare `stty -g` before/after. | Capture `stty -g`; run `cargo run -- -- sh -c 'exit 0'`; press `q`; run `stty -g`; verify values match and terminal echo works. |
| title-proxy.AC7.2: Terminal restores to cooked mode on signal-induced exit | Signal timing and terminal ownership are environment-sensitive without a PTY harness. | Run a long-lived child, send SIGTERM to the proxy, then verify `stty -g` matches the pre-run value and terminal echo/control characters work. |
| title-proxy.AC7.3: Terminal restores to cooked mode on panic | A safe automated panic test needs a PTY harness or a temporary test-only panic trigger that must not remain in production code. | Under a real terminal or dedicated PTY harness, inject a temporary panic immediately after raw mode is enabled, run the proxy, verify `stty -g`/echo restoration, then remove the panic trigger before commit. |
