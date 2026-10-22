---
type: Guide
title: term-preserver
description: Terminal-state PTY proxy, retry loop and composable session launcher
resource: /README.md
status: implemented
author: model:openai/gpt-6-astra
date: 2026-10-22
sources:
  - resource: opencode://ses_e9b53ce88ffeghFZjnj3tsWYCr
    title: Document selectable tta and classic tmux backends and their shared launch surface
  - resource: opencode://ses_e9a9a3c0cffeZZGrwYvVEDokp0
    title: Add the tper alias, Zim-compatible Zsh completion and the candidate protocol
  - resource: /doc/launch/README.md
    title: Feature and runtime launch contract
---

# term-preserver

> 🛟 Keep a terminal window title from reverting when a session detaches.

`term-preserver` is a small, synchronous PTY proxy. It sits between your
terminal and a child process, relaying every byte unchanged while watching the
output stream for OSC title-setting sequences. On top of that foundation it
adds a composable session launcher (shell / ssh / mosh / tmux), a retry prompt
when the child exits, and precise signal handling.

> **Status.** The proxy, launcher, retry prompt, and signal handling all work
> today. The proxy [observes](#observed-state) window title, icon name, and
> terminal color, can [filter](#title-filtering) titles (suppress from the
> stream and/or from internal state), [re-emit](#title-re-emit) captured state,
> and [print](#gone-title-on-exit) a `gone <title>` line on exit. The
> *title-preservation* behavior the name promises — holding a title across
> detach and tagging detached sessions — is still on the [roadmap](#roadmap).

## Install

    cargo install --path .

Two binaries install from the same source: `term-preserver` keeps the original
name, and **`tper`** is the short everyday invocation (`tper --mosh w --tmux
compfuzor`). Both behave identically. From source:

    cargo build --release
    ./target/release/tper --help

## Quick start

Run your default shell through the proxy:

    term-preserver

Run a specific command (everything after `--` is the child, verbatim):

    term-preserver -- bash -l
    term-preserver -- ssh user@host

Reconnect to a remote tmux session over mosh — the headline use case:

    term-preserver --mosh w --tmux compfuzor

When that mosh link drops and the child exits, press space at the prompt to
re-run the *same* resolved command: mosh redials and tmux re-attaches — or, with
the default [session grouping](#session-groups), joins the session's group with
an independent view of the same windows.

## Session launcher

`--tmux` now prefers **tta when available**: exact/unique session matching,
then project-directory matching and configured scaffolding. Choose `--session-backend classic-tmux-cli`
for arbitrary exact-name classic creation. Embedded tta and native `tmax`
implementations are available through optional Cargo features.
See [session launch backends](doc/launch/README.md) for build combinations,
availability rules, shared options, and local/remote behavior.

The launch flags compose a single command **inside-out**, so you don't hand-write
the nested `ssh … -- shell -c 'tmux …'` invocation yourself:

```
job          (optional)   →  trailing startup argv for a new session
  session    (optional)   →  tta or classic tmux selection and attachment
    shell                 →  launch environment and invocation
      transport (optional) →  ssh/mosh wraps the whole thing
```

| Flag | Description |
|------|-------------|
| `--shell <name>` | Launch-shell override (path or basename) |
| `--ssh <host>` | SSH transport (mutually exclusive with `--mosh`) |
| `--mosh <host>` | mosh transport (mutually exclusive with `--ssh`) |
| `--tmux <session>` | Create *or join the session's group* (see [session groups](#session-groups)) |
| `--tta <fragment>` | Require tta session/project matching |
| `--session-backend <value>` | `auto`, `tta-lib`, `tta-cli`, `classic-tmux-cli`, `classic-tmux-tmax` |
| `--session-root <path>` | tta project root |
| `--scaffold[=choice]` | tta generator: default, configured name, or false |
| `--tta-program` / `--tmux-program` | Client executable overrides |
| `--tmux-socket <path>` | Socket on the execution host |
| `--tmux-create <session>` | Create only (`tmux new -s`) |
| `--tmux-attach <session>` | Attach only (`tmux attach-session -t`) |
| `--dir <path>` | Working directory for the innermost layer |
| `--no-login` | Drop login-shell mode (default: login) |
| `--no-interactive` | Drop interactive mode (default: interactive) |
| `--no-tmux-group` | Plain create-or-attach (`tmux new -As`); for tmux < 3.4 or shared view |

`--tta` and the three target-selecting `--tmux*` flags are mutually exclusive,
as are `--ssh`/`--mosh`. A
trailing `-- <command>` becomes the innermost job: it runs *inside* tmux when a
tmux flag is present, otherwise directly under the shell.
With no startup command, tmux chooses its configured default command/shell;
`--shell` controls the launcher environment rather than replacing that default.

If none of these flags are given, the launcher is inert and the legacy
`-- <command>` path (or `$SHELL`) is used unchanged.

### Session groups

Two terminals attached to the *same* tmux session share one current window:
switching windows in one switches them in both, because in tmux the current
window belongs to the session, not to each client. `--tmux` avoids this by
default (tmux ≥ 3.4):

- **First run** creates the suffix-less primary. tta selects a matching project
  or uses its configured scaffold generator on a clean miss;
  classic mode creates the requested exact name.
- **Later runs** resolve the target *at run time* and, when the primary
  exists, join its group instead of attaching: the new session (`dev-1`,
  `dev-2`, …) shares the same windows but keeps its **own current window**, so
  each terminal navigates independently. It also sets
  `destroy-unattached keep-last` on itself, so it quits when its client
  detaches while the group still has members — the primary survives and keeps
  the windows alive across reconnects.

The classic remote CLI path uses this POSIX-shell pattern:

```sh
if tmux has-session -t '=dev' 2>/dev/null; \
   then exec tmux new -t dev \; set destroy-unattached keep-last; \
   else exec tmux new -As dev; fi
```

That probe uses `-t '=dev'` (exact name match) so a lingering `dev-3` satellite
never satisfies it — and the quotes are required for zsh, whose `=command`
expansion would otherwise reject the bare `=dev` word. The condition
re-evaluates on every retry or manual re-run — locally or on the remote host
under ssh/mosh. A trailing `-- <command>` applies to the fresh primary's
initial window only. Local attempts use a launch worker; remote conditional
commands use a POSIX carrier beneath the selected shell. `--no-tmux-group` (or
`[launch] tmux_group = false`) opts out everywhere; `--tmux-attach` remains
the explicit "I want the shared view" flag.

### Launch examples

| You run | Behavior |
|---------|----------|
| `term-preserver` | Run `$SHELL` |
| `--shell bash` | Run `bash -li` |
| `--tmux dev` | Prefer available tta; resolve again on every attempt |
| `--tmux dev --no-tmux-group` | Share the existing session's current window |
| `--tmux dev --session-backend classic-tmux-cli` | Exact-name classic create/group |
| `--mosh w --tmux compfuzor` | Select available CLI backend on `w` |
| `--ssh w --tmux dev` | SSH PTY transport with remote backend selection |
| `--dir ~/src/foo -- make test` | Run the command in the directory through the shell |

Session attempts use the worker or remote availability carrier described in
[launch backends](doc/launch/README.md#attempt-ownership-and-observation).

Notes:

- **ssh vs mosh transport differ.** `--ssh` adds `-t` (ssh suppresses remote PTY
  allocation whenever a command is supplied, and the launcher always supplies
  one — without `-t` the remote shell is degraded and tmux fails with *"open
  terminal failed: not a terminal"*) and hands ssh a *single shell-quoted
  command string*, because ssh concatenates and re-parses the remote command
  through the login shell. `--mosh` passes the argv through with a `--`
  separator and allocates its own PTY, so it needs neither.
- **Shell families.** POSIX shells (bash/zsh/sh/dash) and fish are invoked with
  `-l`/`-i` flags and `exec`. nushell uses `-c` only (no login flag, no `exec`
  builtin); `--no-login`/`--no-interactive` are no-ops for it.
- **Retry re-runs the whole stack.** The resolved command carries the full
  transport + shell + tmux chain, so the [retry prompt](#retry-prompt)
  reconnects and re-attaches with one keystroke.

### What's shell-evaluated vs literal

For session backends, directory paths and startup argv are literal and quoted
across shell/transport boundaries; use `--dir "$HOME/src/foo"` to expand a local
home directory. Shell-only launches retain the original shell-evaluated `cd`
and minimally quoted trailing-command behavior (`tipre-526`, `tipre-ahg`).

## Zsh / Zim completion

The [Zsh module](shell/zsh/functions/_tper) completes **live tmux session names
and immediate project directory basenames** for `--tta`/`--tmux`/
`--tmux-create`/`--tmux-attach` values, plus `--session-backend` (compiled
backends), `--scaffold=` (configured generators), and the other options. Type a
prefix, substring, or wildcard pattern, then press **Tab**:

```text
tper --tmux pipe<Tab>       → tper --tmux pipewire-native
tper --tmux *wire*nat*<Tab> → tper --tmux pipewire-native
tper --session-backend aut<Tab> → tper --session-backend auto
```

Candidates come from the binary itself through a hidden read-only
`--completion-candidates` lane, so they reflect this build's compiled backends
and the same root/socket/program configuration layers a launch uses (including
`TERM_PRESERVER_LAUNCH__*` and `TTA_ROOT`/`PROJECTS_DIR`). Completion observes
only: it never creates, attaches, or switches a session, never starts a server,
and never runs a scaffold generator. The module prefers `tper` on PATH and
falls back to `term-preserver`, so it works whichever name is installed.

First install the binaries with `cargo install --path .`. In your Zim
configuration (`~/.zimrc`, or `$ZIM_CONFIG_FILE`), add the local module
**before** the existing `zmodule completion` line:

```zsh
zmodule ~/src/term-preserver --root shell/zsh
# ...other completion providers...
zmodule completion
```

Run `zimfw install`, then start a new shell. For plain Zsh, put
`fpath=( ~/src/term-preserver/shell/zsh/functions $fpath )` before your
`compinit` call. If Tab is bound to `expand-or-complete`, Zsh may expand a
wildcard against the current directory first; `bindkey '^I' complete-word`
keeps wildcard project completion predictable. Multiple matches stay available
through Zsh's completion UI; inserted names are shell-escaped. Press Tab before
Enter — wildcard matching belongs to completion, while launching uses tta's
literal exact/unique-substring matching.

The [widget tests](tests/completion.rs) drive real Zsh completion in a private
PTY against a private tmux socket under `/tmp/opencode`, plus the candidate
protocol itself across feature combinations.

## Retry prompt

When the child exits, the proxy keeps the screen contents and appends:

    [term-preserver] child exited with code {exit_code}; space=retry q=quit

| Key | Action |
|--------|---------------------------------------------------------|
| space | respawn the same resolved command |
| q | restore the terminal to cooked mode and exit 0 |
| Ctrl+C | restore the terminal and exit 130 |

The proxy stays in raw mode at the prompt, so keystrokes register immediately
without Enter.

## Observed state

As bytes flow from the child, the proxy captures more than the window title.
Each OSC kind updates its own slot:

| OSC | Slot |
|-----|------|
| `0` | window title **and** icon name (same value) |
| `1` | icon name |
| `2` | window title |
| `4;N;SPEC` | palette entry `N` (several may chain in one sequence) |
| `10;SPEC` / `11;SPEC` / `12;SPEC` | default foreground / background / cursor color |

Color specs are parsed to a normalized 24-bit RGB (`rgb:`, `rgbi:`, and `#`
forms, 1–4 hex digits per channel) alongside the raw spec text, so a slot
carries the actual color, not just an opaque string. Captured state is
snapshotted into the exit record for restore and display.

## Title re-emit

By default the proxy captures OSC state in memory but does nothing with it on
child exit. Pass `--reemit-title true` to re-emit all captured state — window
title (OSC 2), icon name (OSC 1), palette (OSC 4), and fg/bg/cursor colors
(OSC 10/11/12) — to the outer terminal when the child dies, reconstructing the
sequences from the raw specs. Useful when a child (like SSH) set a title or
palette you want to keep across reconnections.

## "gone" title on exit

Pass `--print-gone-title true` to print a visible `gone <title> (pid N)` line at
the exit boundary, so you can see what title the exiting process owned. This is
orthogonal to `--reemit-title`: re-emit is the invisible OSC restore, the gone
line is a human-readable record. With no window title captured, no line prints.

## Title filtering

SSH and tmux often stamp the terminal title with `user@host`. The proxy can
suppress those so the title you chose survives. Configure `title_filters` as a
list of regex rules, each with two **independent** actions:

| Action  | Effect |
|---------|--------|
| `strip`  | Drop the matching OSC sequence from the PTY→stdout byte stream — the *outer* terminal's title is never set to the match. |
| `ignore` | Leave the proxy's internal title state untouched, so a later re-emit won't reproduce it. |

The regex is an unanchored search (anchor with `^`/`$` for exact prefixes/suffixes).
A title matching several rules ORs its actions.

```toml
# Never let the outer terminal show your own user@host, and don't remember it.
[[title_filters]]
pattern = "^rektide@"
strip   = true
ignore  = true

# Drop "detached" titles from the stream but keep tracking the last real title.
[[title_filters]]
pattern = "detached"
strip   = true
```

Both flags default to `false`, and the list is empty by default — no filtering.
Filters apply to text title captures only (OSC 0/1/2); icon-name and color
slots are tracked unconditionally and never stripped.

## Signal handling

The proxy separates process-level shutdown from child forwarding.

**Forwarded signals** (`--forward-signals`, default `SIGTERM SIGHUP`) are
delivered to the child's *process group* via `killpg(child_pid, signal)`, so
grandchildren (e.g. a remote shell spawned by ssh) also receive them.

**Shutdown signals** (`--shutdown-signals`, default empty) are forwarded to the
child *and* trigger proxy termination after cleanup. Add a signal here when you
want the proxy to exit once it has forwarded:

    term-preserver --shutdown-signals SIGTERM -- ssh user@host

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
| `--config <path>` | Path to config file | `$XDG_CONFIG_HOME/term-preserver/config.toml` |
| `--forward-signals <sig…>` | Signals forwarded to the child process group | `SIGTERM SIGHUP` |
| `--shutdown-signals <sig…>` | Signals that also shut the proxy down | _(empty)_ |
| `--reemit-title <bool>` | Re-emit captured OSC state (title/icon/color) on child exit | `false` |
| `--print-gone-title <bool>` | Print `gone <title> (pid N)` on child exit | `false` |
| `--shell` / `--ssh` / `--mosh` / `--tmux*` / `--dir` / `--no-login` / `--no-interactive` / `--no-tmux-group` | [Session launcher](#session-launcher) | _none_ |
| `-- <command…>` | Child command (trailing args) | `$SHELL` |

`title_filters` and `[tmux] sources` are configured via the config file (TOML)
or environment only — see [Title filtering](#title-filtering) and
[tmux introspection](#tmux-introspection).

### Config file

`~/.config/term-preserver/config.toml`:

```toml
command = ["bash", "-l"]
forward_signals = ["SIGTERM", "SIGHUP"]
shutdown_signals = []
reemit_title = false
print_gone_title = false

[[title_filters]]
pattern = "^rektide@"
strip = true
ignore = true

[launch]
shell = "zsh"
mosh = "w"
tmux = "compfuzor"
dir = "/home/me/src/foo"
login = true
interactive = true
tmux_group = true

[tmux]
sources = ["poll"]
```

### tmux introspection

When the proxy launches a *local* tmux session (any `--tmux*` flag with no
`--ssh`/`--mosh` transport), it can track the live session and surface its id
on child exit. `[tmux] sources` selects which sources may fill the tmux slot,
so they can be turned on and off independently:

| Source | Effect | Status |
|--------|--------|--------|
| `poll` | Poll the local tmux server every ~20s, correlate by client PID, cache the last sample | implemented |
| `control` | tmux control mode (`tmux -C`) for reactive updates | planned |
| `exit` | One-shot query at child exit | planned (server may be gone) |

Default is `["poll"]`; set `sources = []` to disable all tmux introspection at
runtime. This is independent of the `tmux` Cargo feature, which gates the
sourcing code itself: with the feature off the config is still accepted but
every source is inert (no thread, no `tmux` subprocess). Unknown source names
are rejected at config load.

### Environment variables

Prefix `TERM_PRESERVER_`, with `__` for nesting into `[launch]`. Values are
JSON-encoded where applicable:

    TERM_PRESERVER_COMMAND='["ssh", "user@host"]'
    TERM_PRESERVER_REEMIT_TITLE=true
    TERM_PRESERVER_LAUNCH__MOSH=w
    TERM_PRESERVER_LAUNCH__TMUX=compfuzor
    TERM_PRESERVER_LAUNCH__LOGIN=false
    TERM_PRESERVER_TMUX__SOURCES='["poll"]'

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
  │     ├── ResizeForwarder (SIGWINCH → TIOCSWINSZ)
  │     └── TmuxSampler (local tmux only: polls server by client PID → TmuxState)
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
- **`TmuxSampler`** — per-session, local-tmux-only: polls the tmux server every
  ~20s, correlates the spawned client to its session by PID, and caches the last
  sample in `TmuxState` so the session id / inner command survive child exit.
- **`PromptPolicy`** — pure function mapping prompt keystrokes to retry/exit
  decisions. Testable without spawning the binary.
- **`LaunchConfig`** — configuration for the composable launch flags
  (`--shell`, `--ssh`, `--mosh`, `--tmux`, `--dir`). Shell-only command
  resolution is pure; session workers resolve on each attempt. Also derives a focused
  `LaunchContext` (tmux target + transport) that moves through execution to feed
  the sampler — only a *local* tmux server is reachable by the local `tmux`
  binary, so remote transports yield no sample.

### Module layout

```
src/
  main.rs        term-preserver entry point (delegates to lib bin_entry)
  bin/tper.rs    tper entry point: same shared entry, short install name
  lib.rs         library boundary and the shared binary entry point
  color.rs       ColorSpec + parse_color_spec (rgb:/rgbi:/# → 24-bit)
  completion.rs  read-only completion candidate inventories (--completion-candidates)
  config.rs      figment2 config loading, Config/CliArgs types
  filter.rs      TitleFilterRule (config), FilterSet (compiled), FilterDecision
  launch.rs      LaunchConfig: shell detection, tmux/transport composition, resolution; LaunchContext
  launcher.rs    backend availability, tta adapters, classic CLI/native discovery, retry worker
  runtime.rs     ProxyRuntime, event channel, signal coordinator
  session.rs     ChildSession, RelayThreads, ObservedState/Slots, SessionExit
  proxy.rs       RetryLoop orchestrator
  prompt.rs      PromptPolicy, re-emit/gone formatting (pure)
  signals.rs     SignalPolicy, signal name parsing, process-group forwarding
  terminal.rs    raw mode guard, winsize queries, PTY sizing
  osc.rs         streaming OSC parser (pure state machine), typed OscCapture
  tmux.rs        TmuxSampler/TmuxState: local tmux session introspection by client PID
```

### OSC parser

A streaming state machine that processes bytes one at a time, holding state
across read-buffer boundaries:

```
Ground → ESC → ] → OscParam → ; → OscData → BEL (0x07) or ESC \ → Ground
```

OSC codes 0/1/2 (window title / icon name), 4 (palette), and 10/11/12
(fg/bg/cursor) are recognized and routed to their slots. Captures up to 4096
bytes are kept; longer sequences are ignored to bound memory. The parser also
classifies each byte (emit / hold / flush) so the relay can strip filtered
title sequences from the stream byte-for-byte.

### Design documents

- [`design/tracking/README.md`](design/tracking/README.md) — shell-local tracking research: OSC progress/activity protocols, tmux handling, and per-shell Kopacetic entities
- [`design/notifications/notifications0.glm53m.md`](design/notifications/notifications0.glm53m.md) — proposed observation of OSC 9/777 and kitty OSC 99 notifications (`tipre-notifications`)
- [`design/colors/colors0.glm53m.md`](design/colors/colors0.glm53m.md) — proposed externally readable/writable color state and kitty OSC 21 (`tipre-colors`)
- [`design/terminal-io/coordination0.gpt6a.md`](design/terminal-io/coordination0.gpt6a.md) — shared reply observation, command injection, and query ownership (`tipre-terminal-io`)
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
| `regex` | Title-filter pattern matching |
| `eyre` | Error handling |

## Roadmap

- **preserver mode**: title-preservation state machine — hold a title across detach, reject reverts that don't match the held value. (Title *filtering* — suppressing `user@host` and similar — landed; see [Title filtering](#title-filtering).)
- richer exit dump: live tmux session id + inner process per window (`tipre-tmux-session-id`), sixel graphics slot (`tipre-sixel-slot`).
- tag the title with `[d]` when detached, reset to the original on re-attach
- integrate with `zim-tgo` teleport
- better-defined attached/detached state machine and transitions
- dreaming: materialized arrow cache view of system state
