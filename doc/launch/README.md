---
type: Guide
title: Session launch backends
description: Shared tta and classic tmux launch options, feature selection, automatic availability policy, and retry behavior
resource: /doc/launch/README.md
status: implemented
author: model:openai/gpt-6-astra
date: 2026-10-22
sources:
  - resource: opencode://ses_e80ce13ddffeiWXfWVMBW9qXv5
    title: Integrate shared embedded SSH planning in the local worker, preserve one SSH option set and target shell setup, and verify actual no-target-tta transport with the real proxy
  - resource: opencode://ses_e80e6e9a9ffe2N24sOOfQnrL3D
    title: Migrate embedded tta to the optional runner's shared observation driver and native/process handoff while preserving consumer PTY and retry ownership
  - resource: opencode://ses_e9b53ce88ffeghFZjnj3tsWYCr
    title: Commission and implement embedded/subprocess tta, optional classic CLI/native tmux, shared grouping and scaffold policy, and isolated verification
  - resource: opencode://ses_e9a9a3c0cffeZZGrwYvVEDokp0
    title: Document the completion candidate protocol's backend-aware session inventory
  - resource: /src/launcher.rs
    title: Availability policy and per-attempt launch worker
  - resource: /src/tmux.rs
    title: Shared endpoint and CLI/native observation
  - resource: file:///home/rektide/src/tta/session/launcher/src/plan.rs
    title: Shared grouped launch, initial directory and startup argv planning
  - resource: file:///home/rektide/src/tta/session/launcher/src/scaffold.rs
    title: Shared generator registry, scaffold choice and create-prefix resolution
  - resource: file:///home/rektide/src/tmux-interface-control/README.md
    title: Native command/query boundary and existing-server contract
---

# Session launch backends

`--tmux <target>` defaults to **tta when available**. tta selects an exact
session name, otherwise a unique session substring, otherwise a matching project
under the project root. A clean miss can scaffold a project through tta's configured
generator. Ambiguity and lookup errors stop the attempt. Classic
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
| `tta-lib` | Compose pure `tta-launcher` with `tta-runner`; local or SSH planning without an installed tta on the execution host |
| `tta-cli` | Execute `tta` on the execution host |
| `classic-tmux-cli` | Exact-name classic tmux, with subprocess discovery |
| `classic-tmux-tmax` | Exact-name classic tmux, with native `tmax` discovery |

Both classic implementations enable the `classic-tmux` feature. Omit both to
remove classic launch support; enabling `classic-tmux` alone supplies no backend.
The separate existing `tmux` feature controls introspection, not launching.
Its sample types remain available when observation is disabled.

Default features are `regex,tmux,tta-lib,tta-cli,classic-tmux-cli,classic-tmux-tmax`:
all four launch implementations compile, embedded tta is the first local **and
SSH** choice, the tta CLI remains an explicit SSH option and the existing mosh
carrier, and classic covers create/attach-only flags and availability fallback.
Because `tta-lib` and
`classic-tmux-tmax` resolve sibling path dependencies, default builds need the
sibling checkouts; a crates.io publish would have to drop both from defaults.
Slim builds select backend subsets explicitly:

```sh
# Embedded tta only, classic launch support excluded:
cargo build --no-default-features --features regex,tmux,tta-lib

# Subprocess tta only:
cargo build --no-default-features --features regex,tmux,tta-cli

# Classic native tmux only:
cargo build --no-default-features --features regex,tmux,classic-tmux-tmax
```

These development manifests reference `../tta/session/launcher`,
`../tta/session/runner`, `../tta/shell/command`, and `../tmux-interface-control`
(the `tmax` package).
Keep the sibling checkouts available for Cargo dependency resolution. Native
connections inherit tmax's supported-platform/protocol requirements.

The default `session_backend = "auto"` selects:

1. Embedded tta for local or SSH targets, if compiled.
2. tta CLI, if compiled and the executable is present on the execution host.
3. Local classic native tmux, if compiled.
4. Classic tmux CLI, if compiled.

Local executable detection occurs after the launch shell's startup files. The
embedded SSH path does not check for target tta: it observes target inputs and
prepares commands locally. Explicit SSH `tta-cli` and mosh retain their existing
remote executable carrier. Classic native tmux remains local-only. Embedded
mosh is not yet supported; explicit selection reports an error without making
the existing auto/CLI mosh path unusable.

Fallback concerns **availability only**. A tta matching, permission, protocol,
or execution failure never falls through to classic creation. Explicit backend
selection never falls back. `--tta` requires a tta implementation even in auto
mode, as does explicitly enabling `--scaffold`. `--scaffold=false` permits
classic fallback. `--tmux-create` and `--tmux-attach` retain classic-only semantics.

## Shared options

| Option | Meaning |
|---|---|
| `--session-backend <value>` | `auto` or one of the four implementations above |
| `--session-root <path>` | tta project root; otherwise `TTA_ROOT`, home-relative `PROJECTS_DIR`, then `~/src` |
| `--scaffold[=choice]` | tta generator: `default`, a configured name, or `false`; otherwise `TTA_SCAFFOLD`, then `default` |
| `--tta-program <path>` | tta executable; default `tta` |
| `--tmux-program <path>` | Final tmux client and classic CLI query executable; default `tmux` |
| `--tmux-socket <path>` | Socket on the execution host; local paths are frozen to absolute paths |
| `--dir <path>` | Initial directory; session-launch paths are literal, relative to the execution host's invocation directory |
| `--no-tmux-group` | Attach to the shared existing session instead of creating a grouped satellite |
| `--no-ssh-master` | Disable the managed ssh ControlMaster (`ssh_master`, default on; hosts whose effective ssh config already muxes are tagged along on) |
| `-- <argv...>` | Initial command for a newly created session only |

The embedded and subprocess tta adapters use the same target, grouping, directory
and startup-command surface. `--dir` overrides a selected project's start
directory, rather than supplying a new project match. Existing sessions ignore
startup argv.

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

### Scaffolding

Both adapters use tta's generator registry at `TTA_CONFIG`, otherwise
`$XDG_CONFIG_HOME/tta/config.toml` or `$HOME/.config/tta/config.toml`.
Term-preserver's own `[launch] scaffold` selects a generator; it does not define
another registry. For example, in the **tta** config:

```toml
[scaffold]
default = "rk"

[scaffold.generators.rk]
adapter = "rekon"
```

`--scaffold` uses the configured default (or sole generator),
`--scaffold=rk` selects that named entry, and `--scaffold=false` requires an
existing session/project match. The default is scaffold-enabled in both adapters,
including when the subprocess CLI was built with a different default posture.
Remote launches resolve `TTA_SCAFFOLD` on the remote host.

A generator runs only after an unambiguous clean miss and before creating the
session. Failure stops the launch; an existing session/project never runs the
generator. Missing or ambiguous generator configuration is reported rather than
guessed. `tta-lib` includes both `rekon` and `rekon-native`; the selected CLI must
include the corresponding adapter too. `rekon` calls the user's external script,
whose own project-location behavior still applies. `rekon-native` plans the
configured Git/jj files/remotes/package directly, including target author/license
and Git defaults. See
[tta scaffolding](file:///home/rektide/src/tta/README.md#scaffolding-missing-projects).

The **local** embedded adapter honors `TTA_BACKEND=native|cli`; SSH uses target
tmux CLI observations and a terminal handoff, not local native execution.
`TTA_TMUX_CONFIG` and registry/default inputs belong to the execution host.
Configured term-preserver `tmux_program` (including its default `tmux`) is an
explicit executable choice; `session_root`, scaffold and socket overrides win
when supplied. Remote relative paths remain target-local.

Configuration uses the existing CLI > environment > TOML > defaults layering:

```toml
[launch]
tmux = "term-preserver"
session_backend = "auto"
session_root = "/home/me/src"
scaffold = "default"
tmux_socket = "/run/user/1000/work.sock"
tmux_group = true
ssh_master = true
ssh_master_persist = "10m"

[tmux]
sources = ["poll"]
```

For example, `TERM_PRESERVER_LAUNCH__SESSION_BACKEND=tta-lib` selects embedding.
An unspecified startup command remains unspecified, allowing tmux's configured
default command/shell to run. A bare term-preserver still runs `$SHELL`.

## Attempt ownership and observation

Local launching runs a small worker inside the child PTY, after shell setup.
It resolves the target and replaces itself with the interactive tmux client.
The embedded tta path calls `tta_runner::prepare()` and executes its prepared
plan; shared workflow policy supplies satellite names from a fresh inventory
rather than being reimplemented in this consumer. The CLI path executes tta.
Neither replaces the parent proxy. Retrying starts a new worker, repeating
discovery against current sessions rather than caching a resolved session ID.

For **embedded SSH**, the worker stays local. It passes `--shell`, login and
interactive policy to `TargetSetup`; when no shell was selected it uses target
SHELL, not local SHELL. The same setup applies to every bounded, read-only non-PTY
probe and the final SSH PTY action. Target cwd/HOME/UID/root/config/registry/Git
facts supply defaults, with explicit consumer settings winning. A fresh remote
terminal attaches and cannot inherit authority to switch a local tmux client.
No target tta is needed, including cold creation. The worker hands the prepared
command to SSH; the parent retains its proxy, PTY and retry prompt.

The parent's `ssh_control_options()` result is serialized once into the attempt
request and shared unchanged by probes and handoff, along with the same OpenSSH
config. No silently different connection/master policy is introduced. Startup
noise, command diagnostics/status, malformed or truncated replies and capture
incompleteness remain distinct. Read failure stops before mutation; a failed
scaffold/create prevents handoff. Grouped create → attach → `keep-last` remains
one tmux queue. After action start, transport loss may leave target mutation
uncertain: no library fallback/replay/rollback/prune occurs. A **user-authorized**
retry starts a new worker and fresh discovery, not the previous mutation block.

The worker-free target profile uses POSIX sh/Unix utilities and GNU find; it adds
no Python/Perl/tta worker requirement. See the
[tta SSH guide](file:///home/rektide/src/tta/README.md#ssh-targets) for the precise
profile and UTF-8 boundaries, the
[actual SSH toolkit](file:///home/rektide/src/tta/verification/ssh/README.md) for
reusable no-target-tta evidence, and the
[library-side consumer account](file:///home/rektide/src/tta/doc/consumer/term-preserver.md).

**Mosh remains on its existing remote CLI carrier.** It is not an exec/probe API.
Embedded planning needs an explicitly selected SSH observation channel (host,
options/master policy, and matching target setup) before a mosh handoff. That
integration choice is deferred, not silently approximated by mosh probes.

Native `tmax` owns server queries; the ordinary tmux client owns interactive
terminal attachment and cold-server startup. Native classic discovery chooses
the grouped-session target; shared observation uses native queries when that
backend is selected. Explicit CLI remote backends and mosh retain their carriers;
embedded SSH uses the prepared handoff described above.

The local sampler uses the launch socket and client executable. Client PID is
the strongest correlation. tta fragments are never treated as exact fallback
session names; auto launches also use PID-only correlation. Failed queries retain
the last complete sample. The polling interval remains approximately 20 seconds,
so a short-lived attempt can finish before it is sampled.

## Shell completion

The hidden `--completion-candidates {backends,projects,sessions,scaffolds}`
lane feeds the [Zsh/Zim module](/README.md#zsh--zim-completion) and observes
the same merged launch configuration, including an explicit `--config` and
program overrides. Session inventories follow the effective backend exactly
as this guide resolves it — an explicit `tta-cli` asks the installed tta's
own candidate protocol even when the embedded registry is compiled:
embedded native or `TTA_BACKEND=cli` lookup for `tta-lib`, native or CLI
discovery for the classic backends, with the auto availability order above.
An absent server is an empty inventory — a query never starts one — and
errors go to stderr leaving stdout empty so widgets can silence them.
Inventories are local-only: `--ssh`/`--mosh` answer with no host inventory
(and zero process contact) rather than querying local state a remote launch
would never use; the compiled backend list stays available everywhere
because it never contacts a host.

## Related accounts

- [Main guide](/README.md#session-launcher): entrypoint and proxy behavior.
- [Original launcher design](/doc/design/2026-06-22-session-launcher.md): historical
  shell/transport composition that these backend adapters extend.
- [tta guide](file:///home/rektide/src/tta/README.md): matching, grouped switching,
  and the shared library/CLI launch contract.
