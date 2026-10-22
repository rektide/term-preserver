---
type: Guide
title: Session launch backends
description: Shared tta and classic tmux launch options, feature selection, automatic availability policy, and retry behavior
resource: /doc/launch/README.md
status: implemented
author: model:openai/gpt-6-astra
date: 2026-10-22
sources:
  - resource: opencode://ses_e9b53ce88ffeghFZjnj3tsWYCr
    title: Commission and implement embedded/subprocess tta, optional classic CLI/native tmux, upstream grouping parity and isolated verification
  - resource: /src/launcher.rs
    title: Availability policy and per-attempt launch worker
  - resource: /src/tmux.rs
    title: Shared endpoint and CLI/native observation
  - resource: file:///home/rektide/src/tta/session/launcher/src/plan.rs
    title: Shared grouped launch, initial directory and startup argv planning
  - resource: file:///home/rektide/src/tmux-interface-control/README.md
    title: Native command/query boundary and existing-server contract
---

# Session launch backends

`--tmux <target>` defaults to **tta when available**. tta selects an exact
session name, otherwise a unique session substring, otherwise a matching project
under the project root. Ambiguity and lookup errors stop the attempt. Classic
tmux instead creates or joins an arbitrary exact session name.

```sh
term-preserver --tmux term-preserver
term-preserver --tta term-preserver --session-backend tta-lib
term-preserver --tmux dev --session-backend classic-tmux-cli
term-preserver --tmux dev --session-backend classic-tmux-tmax
term-preserver --mosh w --tmux compfuzor
```

## Features and runtime selection

| Cargo feature / `--session-backend` value | Implementation |
|---|---|
| `tta-lib` | Embed the sibling `tta-launcher`; no installed tta executable required |
| `tta-cli` | Execute `tta` on the execution host |
| `classic-tmux-cli` | Exact-name classic tmux, with subprocess discovery |
| `classic-tmux-tmax` | Exact-name classic tmux, with native `tmax` discovery |

Both classic implementations enable the `classic-tmux` feature. Omit both to
remove classic launch support; enabling `classic-tmux` alone supplies no backend.
The separate existing `tmux` feature controls introspection, not launching.
Its sample types remain available when observation is disabled.

Default features are `regex,tmux,tta-cli,classic-tmux-cli`. The native/embedded
implementations are opt-in builds:

```sh
# All four implementations, with runtime selection:
cargo build --all-features

# Embedded tta only: classic launch support excluded:
cargo build --no-default-features --features regex,tmux,tta-lib

# Subprocess tta only:
cargo build --no-default-features --features regex,tmux,tta-cli

# Classic native tmux only:
cargo build --no-default-features --features regex,tmux,classic-tmux-tmax
```

These development manifests reference `../tta/session/launcher`,
`../tta/shell/command`, and `../tmux-interface-control` (the `tmax` package).
Keep the sibling checkouts available for Cargo dependency resolution. Native
connections inherit tmax's supported-platform/protocol requirements.

The default `session_backend = "auto"` selects:

1. Local embedded tta, if compiled.
2. tta CLI, if compiled and the executable is present on the execution host.
3. Local classic native tmux, if compiled.
4. Classic tmux CLI, if compiled.

Local executable detection occurs after the launch shell's startup files.
SSH/mosh perform their executable check remotely, on every attempt. Embedded
and native launch implementations require a local server; selecting either
explicitly with a transport reports an error.

Fallback concerns **availability only**. A tta matching, permission, protocol,
or execution failure never falls through to classic creation. Explicit backend
selection never falls back. `--tta` requires a tta implementation even in auto
mode. `--tmux-create` and `--tmux-attach` retain classic-only semantics.

## Shared options

| Option | Meaning |
|---|---|
| `--session-backend <value>` | `auto` or one of the four implementations above |
| `--session-root <path>` | tta project root; otherwise `TTA_ROOT`, home-relative `PROJECTS_DIR`, then `~/src` |
| `--tta-program <path>` | tta executable; default `tta` |
| `--tmux-program <path>` | Final tmux client and classic CLI query executable; default `tmux` |
| `--tmux-socket <path>` | Socket on the execution host; local paths are frozen to absolute paths |
| `--dir <path>` | Initial directory; session-launch paths are literal, relative to the execution host's invocation directory |
| `--no-tmux-group` | Attach to the shared existing session instead of creating a grouped satellite |
| `-- <argv...>` | Initial command for a newly created session only |

The embedded and subprocess tta adapters use the same target, grouping, directory
and startup-command surface. tta still requires a session or project match;
`--dir` overrides a selected project's start directory, rather than supplying
a new project match. Existing sessions ignore startup argv.

Grouping is on by default in term-preserver and requires tmux 3.4+. Satellites
share windows but have independent current windows, and use
`destroy-unattached keep-last`. tta also handles invocation from inside tmux by
switching the current client, including grouped switching.

tta's CLI must include the new `--group`, `--dir`, and startup-argv support.
Use a rebuilt sibling binary while developing:

```sh
term-preserver --tta term-preserver --session-backend tta-cli \
  --tta-program ../tta/target/debug/tta
```

The embedded tta adapter also honors `TTA_BACKEND=native|cli` and
`TTA_TMUX_CONFIG`; the subprocess inherits these tta variables. The explicit
term-preserver executable and local socket settings own endpoint selection.

Configuration uses the existing CLI > environment > TOML > defaults layering:

```toml
[launch]
tmux = "term-preserver"
session_backend = "auto"
session_root = "/home/me/src"
tmux_socket = "/run/user/1000/work.sock"
tmux_group = true

[tmux]
sources = ["poll"]
```

For example, `TERM_PRESERVER_LAUNCH__SESSION_BACKEND=tta-lib` selects embedding.
An unspecified startup command remains unspecified, allowing tmux's configured
default command/shell to run. A bare term-preserver still runs `$SHELL`.

## Attempt ownership and observation

Local launching runs a small worker inside the child PTY, after shell setup.
It resolves the target and replaces itself with the interactive tmux client.
The embedded tta path calls the library directly; the CLI path executes tta.
Neither replaces the parent proxy. Retrying starts a new worker, repeating
discovery against current sessions rather than caching a resolved session ID.

Native `tmax` owns server queries; the ordinary tmux client owns interactive
terminal attachment and cold-server startup. Native classic discovery chooses
the grouped-session target; shared observation uses native queries when that
backend is selected. Remote transports wrap a remote CLI launch instead.

The local sampler uses the launch socket and client executable. Client PID is
the strongest correlation. tta fragments are never treated as exact fallback
session names; auto launches also use PID-only correlation. Failed queries retain
the last complete sample. The polling interval remains approximately 20 seconds,
so a short-lived attempt can finish before it is sampled.

## Related accounts

- [Main guide](/README.md#session-launcher): entrypoint and proxy behavior.
- [Original launcher design](/doc/design/2026-06-22-session-launcher.md): historical
  shell/transport composition that these backend adapters extend.
- [tta guide](file:///home/rektide/src/tta/README.md): matching, grouped switching,
  and the shared library/CLI launch contract.
