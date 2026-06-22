# title-preserver

> 🛟 Keep a terminal window title from reverting when a session detaches.

`title-preserver` is a small, synchronous PTY proxy. It sits between your
terminal and a child process, relaying every byte unchanged while watching the
output stream for OSC title-setting sequences. On top of that foundation it
adds a composable session launcher (shell / ssh / mosh / tmux), a retry prompt
when the child exits, and precise signal handling.

> **Status.** The proxy, launcher, retry prompt, and signal handling all work
> today. The *title-preservation* behavior the name promises — holding a title
> across detach and tagging detached sessions — is still on the [roadmap](#roadmap).
> Right now titles are observed and (optionally) re-emitted, not yet filtered.

## Install

    cargo install --path .

Or build and run from source:

    cargo build --release
    ./target/release/title-preserver --help

## Quick start

Run your default shell through the proxy:

    title-preserver

Run a specific command (everything after `--` is the child, verbatim):

    title-preserver -- bash -l
    title-preserver -- ssh user@host

Reconnect to a remote tmux session over mosh — the headline use case:

    title-preserver --mosh w --tmux compfuzor

When that mosh link drops and the child exits, press space at the prompt to
re-run the *same* resolved command: mosh redials and tmux re-attaches.

## Session launcher

The launch flags compose a single command **inside-out**, so you don't hand-write
the nested `ssh … -- shell -c 'tmux …'` invocation yourself:

```
shell        (innermost)   →  which shell binary, and how it's invoked
  tmux       (optional)    →  wrap the shell's job in a tmux session
    transport (optional)   →  ssh/mosh wraps the whole thing
```

| Flag | Description |
|------|-------------|
| `--shell <name>` | Shell override (path or basename) |
| `--ssh <host>` | SSH transport (mutually exclusive with `--mosh`) |
| `--mosh <host>` | mosh transport (mutually exclusive with `--ssh`) |
| `--tmux <session>` | Create *or* attach (`tmux new -As`) |
| `--tmux-create <session>` | Create only (`tmux new -s`) |
| `--tmux-attach <session>` | Attach only (`tmux attach-session -t`) |
| `--dir <path>` | Working directory for the innermost layer |
| `--no-login` | Drop login-shell mode (default: login) |
| `--no-interactive` | Drop interactive mode (default: interactive) |

The three `--tmux*` flags are mutually exclusive, as are `--ssh`/`--mosh`. A
trailing `-- <command>` becomes the innermost job: it runs *inside* tmux when a
tmux flag is present, otherwise directly under the shell.

If none of these flags are given, the launcher is inert and the legacy
`-- <command>` path (or `$SHELL`) is used unchanged.

### What each invocation resolves to

| You run | Resolved command |
|---------|------------------|
| `title-preserver` | `zsh -li` |
| `--shell bash` | `bash -li` |
| `--tmux dev` | `zsh -li -c 'exec tmux new -As dev'` |
| `--mosh w --tmux compfuzor` | `mosh w -- zsh -li -c 'exec tmux new -As compfuzor'` |
| `--ssh w --tmux dev` | `ssh -t w -- zsh -li -c 'exec tmux new -As dev'` |
| `--mosh w --shell nu --tmux dev` | `mosh w -- nu -c 'tmux new -As dev'` |
| `--dir ~/src/foo` | `zsh -c 'cd ~/src/foo && exec zsh -li'` |
| `--dir ~/src/foo -- make test` | `zsh -li -c 'cd ~/src/foo && exec make test'` |
| `--ssh w` | `ssh -t w -- zsh -li` |

(`zsh` above is whatever resolves from `--shell`, then `$SHELL`, then a runtime
fallback that prefers zsh and finally `sh`.)

Notes:

- **`--ssh` always adds `-t`.** ssh suppresses remote PTY allocation whenever a
  command is supplied, and the launcher always supplies one — without `-t` an
  interactive remote shell is degraded and tmux fails with *"open terminal
  failed: not a terminal."* mosh allocates a PTY itself.
- **Shell families.** POSIX shells (bash/zsh/sh/dash) and fish are invoked with
  `-l`/`-i` flags and `exec`. nushell uses `-c` only (no login flag, no `exec`
  builtin); `--no-login`/`--no-interactive` are no-ops for it.
- **Retry re-runs the whole stack.** The resolved command carries the full
  transport + shell + tmux chain, so the [retry prompt](#retry-prompt)
  reconnects and re-attaches with one keystroke.

### What's shell-evaluated vs literal

`--dir` is **evaluated by the shell** (`cd <path>`), so `~`, `$VAR`, and globs
expand as you'd expect — but a path containing spaces currently needs manual
escaping. Trailing command arguments and tmux session names are passed through
with minimal quoting today; arguments containing spaces or shell metacharacters
may not survive intact. Tightening this is tracked in the issue tracker
(`tipre-526`, `tipre-ahg`).

## Retry prompt

When the child exits, the proxy keeps the screen contents and appends:

    [title-preserver] child exited with code {exit_code}; space=retry q=quit

| Key | Action |
|--------|---------------------------------------------------------|
| space | respawn the same resolved command |
| q | restore the terminal to cooked mode and exit 0 |
| Ctrl+C | restore the terminal and exit 130 |

The proxy stays in raw mode at the prompt, so keystrokes register immediately
without Enter.

## Title re-emit

By default the proxy captures OSC title sequences in memory but does nothing
with them on child exit. Pass `--reemit-title true` to re-emit the last captured
title to the outer terminal when the child dies — useful when a child (like SSH)
set a title you want to keep across reconnections.

## Signal handling

The proxy separates process-level shutdown from child forwarding.

**Forwarded signals** (`--forward-signals`, default `SIGTERM SIGHUP`) are
delivered to the child's *process group* via `killpg(child_pid, signal)`, so
grandchildren (e.g. a remote shell spawned by ssh) also receive them.

**Shutdown signals** (`--shutdown-signals`, default empty) are forwarded to the
child *and* trigger proxy termination after cleanup. Add a signal here when you
want the proxy to exit once it has forwarded:

    title-preserver --shutdown-signals SIGTERM -- ssh user@host

With the default empty list, `SIGTERM` during an active child is forwarded
without shutting the proxy down. At the retry prompt (no active child),
`SIGTERM`/`SIGHUP` always trigger shutdown with terminal restoration.

**Other signals:**

- `SIGWINCH` — terminal resizes propagate to the child PTY via `ioctl(TIOCSWINSZ)`.
- `SIGINT`, `SIGQUIT`, `SIGTSTP` — generated by the PTY's line discipline
  inside the child automatically; no forwarding needed.

## Configuration

Config loads via [figment2](https://docs.rs/figment2) with precedence:
**CLI args > environment variables > TOML file > defaults**.

### CLI options

| Option | Description | Default |
|--------|-------------|---------|
| `--config <path>` | Path to config file | `$XDG_CONFIG_HOME/title-preserver/config.toml` |
| `--forward-signals <sig…>` | Signals forwarded to the child process group | `SIGTERM SIGHUP` |
| `--shutdown-signals <sig…>` | Signals that also shut the proxy down | _(empty)_ |
| `--reemit-title <bool>` | Re-emit captured OSC title on child exit | `false` |
| `--shell` / `--ssh` / `--mosh` / `--tmux*` / `--dir` / `--no-login` / `--no-interactive` | [Session launcher](#session-launcher) | _none_ |
| `-- <command…>` | Child command (trailing args) | `$SHELL` |

### Config file

`~/.config/title-preserver/config.toml`:

```toml
command = ["bash", "-l"]
forward_signals = ["SIGTERM", "SIGHUP"]
shutdown_signals = []
reemit_title = false

[launch]
shell = "zsh"
mosh = "w"
tmux = "compfuzor"
dir = "~/src/foo"
login = true
interactive = true
```

### Environment variables

Prefix `TITLE_PRESERVER_`, with `__` for nesting into `[launch]`. Values are
JSON-encoded where applicable:

    TITLE_PRESERVER_COMMAND='["ssh", "user@host"]'
    TITLE_PRESERVER_REEMIT_TITLE=true
    TITLE_PRESERVER_LAUNCH__MOSH=w
    TITLE_PRESERVER_LAUNCH__TMUX=compfuzor
    TITLE_PRESERVER_LAUNCH__LOGIN=false

## Architecture

Synchronous throughout — no async runtime, no Tokio. Two blocking `std::thread`
relay workers move bytes. The design prioritizes being small and easy to reason
about.

### Data plane

```
stdin ──► stdin relay ──► PTY master ──► child process
                                          │
child process ──► PTY master ──► stdout relay + title observer ──► stdout
```

Bytes pass through unchanged. The OSC parser observes the PTY-to-stdout stream
but never modifies it.

### Control plane

```
ProxyRuntime
  ├── raw terminal mode (entered once, restored on exit/panic/signal)
  ├── signal thread (receives OS signals, applies SignalPolicy)
  ├── event channel (ProxyEvent: ChildExited, SignalReceived, PromptKey, ShutdownRequested)
  └── panic hook (restores saved termios before chaining)
        │
        ▼
RetryLoop
  ├── ChildSession::spawn → wait → SessionExit
  │     ├── RelayThreads (stdin→PTY, PTY→stdout + title observer)
  │     ├── SignalForwarder (process-group forwarding via runtime)
  │     └── ResizeForwarder (SIGWINCH → TIOCSWINSZ)
  └── PromptPolicy (pure: byte → Retry/Exit/Ignore decision)
```

Each resource with a distinct lifetime gets a small owner:

- **`ProxyRuntime`** — process-level: raw mode, signal coordination, shutdown
  state, panic restoration.
- **`ChildSession`** — one PTY child attempt: open PTY, spawn child, dup fds,
  start relays and forwarders, wait, join relays, produce `SessionExit`.
- **`RelayThreads`** — owns the two blocking relay workers and classifies relay
  completion (normal EOF, expected PTY close/EIO, stdin cancellation, panic).
- **`SignalForwarder`** — per-session child process-group forwarding.
- **`ResizeForwarder`** — per-session `SIGWINCH` propagation to the child PTY.
- **`PromptPolicy`** — pure function mapping prompt keystrokes to retry/exit
  decisions. Testable without spawning the binary.
- **`LaunchConfig`** — pure pipeline resolving the composable launch flags
  (`--shell`, `--ssh`, `--mosh`, `--tmux`, `--dir`) into a `CommandSpec`. Reads
  no I/O; `$SHELL` is injected by the caller.

### Module layout

```
src/
  main.rs        entry point: CLI parse, config load, delegate to library
  lib.rs         library boundary
  config.rs      figment2 config loading, Config/CliArgs types
  launch.rs      LaunchConfig: shell detection, tmux/transport composition, resolution
  runtime.rs     ProxyRuntime, event channel, signal coordinator
  session.rs     ChildSession, RelayThreads, forwarders, CommandSpec, SessionExit
  proxy.rs       RetryLoop orchestrator
  prompt.rs      PromptPolicy, prompt formatting (pure)
  signals.rs     SignalPolicy, signal name parsing, process-group forwarding
  terminal.rs    raw mode guard, winsize queries, PTY sizing
  osc.rs         streaming OSC 0/1/2 title parser (pure state machine)
```

### OSC title parser

A streaming state machine that processes bytes one at a time, holding state
across read-buffer boundaries:

```
Ground → ESC → ] → OscParam → ; → OscData → BEL (0x07) or ESC \ → Ground
```

OSC codes 0, 1, and 2 are recognized as title-setting sequences. Titles up to
4096 bytes are captured; longer sequences are ignored to bound memory.

### Design documents

- [`doc/design/2026-06-08-title-proxy.md`](doc/design/2026-06-08-title-proxy.md) — original MVP design
- [`doc/design/2026-06-08-child-session-orchestration.md`](doc/design/2026-06-08-child-session-orchestration.md) — ownership refactor and event coordinator
- [`doc/design/2026-06-22-session-launcher.md`](doc/design/2026-06-22-session-launcher.md) — composable session launching flags

## Dependencies

| Crate | Purpose |
|-------|---------|
| `pty-process` | PTY management: open, spawn, resize |
| `nix` | Termios (raw mode), ioctl (window size), process signals |
| `signal-hook` | Signal registration and iteration |
| `figment2` | Layered configuration (CLI > env > file > defaults) |
| `clap` | CLI argument parsing |
| `bon` | Builder pattern on the config struct |
| `eyre` | Error handling |

## Roadmap

- **preserver mode**: filter title updates to reject/ignore reverts
- tag the title with `[d]` when detached, reset to the original on re-attach
- integrate with `zim-tgo` teleport
- better-defined attached/detached state machine and transitions
- dreaming: materialized arrow cache view of system state
