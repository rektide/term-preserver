---
type: Design
title: Shell-local tracking with Kopacetic entities
description: A lightweight shell-entry mode and current-state publication boundary for term-preserver.
resource: /design/tracking/shell-tracking0.gpt6a.md
status: draft
author: model:openai/gpt-6-astra
date: 2026-10-14
sources:
  - resource: "opencode://ses_ec84404aaffeHUijDXZqczAc1F"
    title: "User direction and source inspection for shell-local tracking; research and design commissioned before implementation"
  - resource: /doc/design/2026-06-08-title-proxy.md
    title: Original Atuin PTY-proxy inspiration and lightweight relay design
  - resource: /doc/design/2026-06-08-child-session-orchestration.md
    title: Process, child-session, relay, and terminal ownership boundaries
  - resource: /src/osc.rs
    title: Current typed OSC captures and streaming parser
  - resource: /src/session.rs
    title: In-memory slots, filtering, PTY spawning, relay, and exit snapshot
  - resource: /src/config.rs
    title: Default command follows SHELL and config layering
  - resource: /src/launch.rs
    title: Shell resolution and command composition
  - resource: /src/proxy.rs
    title: Unconditional retry-prompt lifecycle
  - resource: /src/main.rs
    title: Current CLI entry path
  - resource: /src/tmux.rs
    title: Local tmux sampling boundary
  - resource: /src/color.rs
    title: Existing raw and normalized color representation
  - resource: "file:///home/rektide/src/facet-format-path/src/projection/serializer.rs"
    title: Inspected map-key projection requires semantic string keys
  - resource: "https://github.com/rektide/kopacetic/tree/5090da3eae1915848d550425740a317fb11671f5"
    title: "Local Kopacetic source inspected: README, core entity types, Turso storage/schema, Rkyv and Caminus READMEs"
  - resource: "https://github.com/atuinsh/atuin/tree/6896db72fa3e56f996738c4aef02d250cfa24ee1/crates/atuin-pty-proxy"
    title: Archived Atuin crate manifest and osc133.rs inspected
verification:
  method: Source inspection; proposed behavior is not implemented or runtime-tested
---

# Shell-local tracking with Kopacetic entities

## Direction

Give each observed shell lifetime a Kopacetic entity containing its current
terminal metadata and lifecycle. Run the proxy immediately around zsh, including
inside tmux panes, so observation happens before a multiplexer or transport can
consume the metadata. Retain the connection-launcher use case as another lifecycle
policy over the same PTY machinery.

The user selected **research and design first**. CLI names and schema below are
proposals. Historical storage and historical views belong to Kopacetic; the proxy
produces meaningful state updates and process-lifecycle facts.

## Verified starting point

The original design explicitly credits **Atuin's `atuin-pty-proxy`**, particularly
its relay threads and `osc133.rs` parser architecture. The current project uses
`pty-process`, rather than Atuin's `portable-pty`, and its own observer. This is
architectural inspiration, not an Atuin dependency. The inspected material does
not establish a separate dependency or lineage called “rtx.”

Current capture cases in `src/osc.rs:297–395`:

| OSC | Recorded effect |
| --- | --- |
| 0 / 1 / 2 | Both title and icon / icon / window title |
| 4 / 5 | Indexed palette / special colors |
| 10 / 11 / 12 | Foreground / background / cursor color |
| 104 / 105 | Reset all or selected palette / special colors |
| 110 / 111 / 112 | Reset foreground / background / cursor color |

OSC 9;4, 7501, 7, and 133 are not recorded. The observer accepts seven-bit OSC
introducers and BEL or ESC-backslash termination. Ordinary completed unknown
sequences pass through. Title filters can independently strip the output sequence
and ignore the state update; they apply to all text captures, including OSC 1.
The README's narrower inventory and unconditional-icon wording lag the code.

`ObservedState` retains latest values in memory, then `SessionExit` takes a
snapshot. There is no durable registration or change history. A separate local
tmux sampler supplies session/pane information approximately every 20 seconds.

## Two invocation surfaces, one PTY engine

Recommended interface:

- Normal `term-preserver`: connection/command CLI, with its existing retry policy.
- A dedicated installed **`term-preserver-shell`** entry point: shell-compatible
  arguments and shell lifecycle, sharing the library. This is the path that can
  be selected as a terminal shell or login shell.
- An explicit one-shot mode in the normal CLI can expose the same lifecycle for
  testing and command wrappers. Tracking itself is independently configurable,
  so reconnecting sessions can publish state too.

A dedicated entry point avoids interpreting shell flags as proxy flags. A passwd
shell entry is an executable path, not a place to append `--mode` arguments.

### Shell-entry contract

1. Configure the inner shell explicitly, initially `/bin/zsh` (or its actual
   installed absolute path). Never resolve it from `$SHELL` when that names the
   proxy. Detect self-execution through aliases/symlinks as well as literal paths.
2. Translate login invocation (`argv[0]` beginning with `-`) and preserve shell
   arguments such as `-l`, `-i`, and `-c`, including the exact command string and
   arguments following it. Do not reuse `trailing.join(" ")` for this path.
3. With a terminal, use the PTY engine and let zsh determine interactivity unless
   explicitly requested. With non-TTY invocation, execute the real shell with
   inherited streams; this preserves pipelines and remote `shell -c` operation.
   That direct-exec path does not provide PTY-derived tracking.
4. On shell exit, publish the final snapshot, restore the terminal, and return
   the child's status. No retry prompt or extra stdout output in shell mode.
5. Use shell-appropriate shutdown handling rather than falling into reconnect
   behavior. Verify hangup/termination, resize, foreground job control, and drain
   ordering against a real outer PTY.
6. Set the child's `$SHELL` to the real inner shell as the proposed default, so
   ordinary nested shell launches do not accidentally wrap themselves. Explicit
   nested proxy launches may create their own entities with a parent link.

There is a second existing resolution trap: `Config.command` defaults to
`[$SHELL]`, then an active launcher can treat that default as an explicit inner
command. Shell mode needs an actual distinction between “no command supplied” and
“run this command,” not just a different fallback executable.

## Entity identity and state

Use `Entity::standalone("TerminalShell", shell_id, payload)` with
`kopacetic-core`'s default Kubernetes feature disabled. Allocate a unique ID per
observed shell lifetime; PID, title, and tmux pane number are attributes, not the
identity. One publisher owns an entity. A restarted shell gets a new ID.

Suggested payload domains:

| Domain | Contents |
| --- | --- |
| Process | Host identity, proxy PID, child PID, process-start discriminator, executable, optional parent shell ID |
| Lifecycle | Starting/running/exited, start/update/exit timestamps, exit status, per-entity observation sequence |
| Terminal | Existing title/icon/color slots; dedicated OSC 9;4 and OSC 7501 state |
| Context | Observation source, optional tmux pane/session relationship, optional connection target |

An outer `term-preserver -> ssh/mosh/tmux` wrapper describes a connection or
observed terminal stream. It cannot invent one entity for every remote shell.
Use a distinct entity kind for that wrapper, or an explicit scope discriminator.
True per-shell entities are created by proxies running around those shells.

Represent **unobserved** separately from a protocol's explicit clear/idle/hidden
state. Retain each protocol independently and derive any combined UI status later.
A child exit updates lifecycle; it does not rewrite the last progress observation
to “complete.” A leftover running record after an abrupt death is a last report,
not proof that its process remains alive. No automatic record deletion is needed.

## Publication boundary

The byte path should remain small:

```text
PTY bytes -> streaming parser -> typed update -> state reducer -> stdout relay
                                              |
                                              v
                                    publication worker -> Kopacetic
```

Parse and validate the complete protocol message before changing state. Apply
updates in stream order and publish accepted state, independently of presentation
filtering. For titles, keep the existing `ignore` semantics explicit. Storage I/O
belongs on a worker, outside the relay and outside the observed-state mutex.

For a latest-state contract, a bounded dirty-state handoff can coalesce updates
while a write is in progress. The worker then publishes the newest complete state.
Record an observation sequence and timestamps so readers can identify recency.
Publish initial registration, changes, and a final snapshot after output drainage.
Keep publication failure visible through diagnostics while preserving shell I/O;
retain the latest dirty snapshot for a later retry. Final-flush waiting must be
bounded so storage trouble cannot indefinitely prevent shell exit.

**History consequence:** coalescing discards intermediate observations before
Kopacetic sees them. A future history backend can retain submitted revisions, but
cannot recover discarded updates or reconstruct this session's overwritten past.
If every transition becomes a requirement, agree a delivery/overflow contract with
Kopacetic then. The present design promises latest state, not a lossless event log.

## Kopacetic backend choice

The inspected checkout is `5090da3eae1915848d550425740a317fb11671f5`;
its only visible local modification was unrelated Beads data. Relevant API facts:

| Backend | Fit and tradeoff |
| --- | --- |
| `kopacetic-rkyv` | Synchronous identity-addressed complete snapshots with explicit schema revision and atomic replacement. Good lightweight initial writer; no public listing/query index, watcher, or historical store. |
| `kopacetic-turso` | Preferred SQL backend; `put` replaces an entity and path rows transactionally, `connection()` enables queries across entities. Async API, with driver/concurrency limits documented by its owner. |
| `kopacetic-caminus` | Inspectable field-per-file state and access to low-level watch/history/VCS policies. Complete entity updates are not atomic; the filesystem representation constrains payload keys. |

For **just durable state on each shell**, start with Rkyv behind one narrow
publisher adapter. For **listing/searching all registered shells immediately**,
Turso is the better existing substrate. Rkyv's missing collection discovery should
not turn into a second home-grown registry in term-preserver. This is the material
backend decision still to make; the entity model works for either.

The persistent payload should derive Facet deliberately. Existing Serde derives
do not satisfy this requirement, and schema changes need explicit revision and
migration decisions. Keep revisions and history/query evolution with Kopacetic.

One concrete adapter requirement: current palette/special slots are
`BTreeMap<u32, ColorSpec>`, but the inspected path projector accepts string map
keys. Persist a sorted sequence of `{ index: u32, color: ... }` entries (or an
explicit canonical string-key adapter), rather than assuming a Facet derive makes
the current slot layout directly storable. Preserve both `ColorSpec.raw` and its
optional normalized RGB. This small persistent DTO also isolates schema revisions
from changes to relay-internal types.

## Coherent implementation slices after discussion

1. Typed protocol updates and reducers, preserving independent slots and wire
   pass-through. Test terminators, fragmentation, clear/default semantics,
   malformed input, and repeated/state-only updates against the protocol brief.
2. Shell entry and one-shot lifecycle, with exact argv forwarding, self-recursion
   prevention, non-TTY behavior, exit status, and PTY restoration tests.
3. Per-shell entity publication through the selected Kopacetic adapter. Verify
   two concurrent shells remain distinct, reopening restores current state,
   resets persist, failure does not stall relay I/O, and final output precedes
   the exit snapshot.

Group new code by concern, for example `tracking/{state,publisher}.rs` and
`shell/{entry,invocation}.rs`, alongside the existing PTY/session modules. The
existing OSC parser can remain the framing owner while protocol-specific decoding
grows beneath it. Avoid duplicating reductions between observer-private state and
shared slots when extending the current parser.

## Cross-references

- [Original proxy design](../../doc/design/2026-06-08-title-proxy.md): evidence
  for the Atuin connection and intentionally lightweight origin.
- [Child-session orchestration](../../doc/design/2026-06-08-child-session-orchestration.md):
  reuse its ownership boundaries for the publication worker and final snapshot.
- [Session launcher](../../doc/design/2026-06-22-session-launcher.md): connection
  composition remains useful, while shell-entry argv semantics need their own path.
- [Tracking research index](README.md): companion protocol and tmux evidence,
  including which side of the multiplexer can observe a shell's own metadata.
