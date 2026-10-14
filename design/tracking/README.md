---
type: Index
title: Shell and terminal-state tracking research
description: Protocol evidence and a shell-local Kopacetic publication design.
resource: /design/tracking/README.md
status: draft
author: model:openai/gpt-6-astra
date: 2026-10-14
sources:
  - resource: "opencode://ses_ec84404aaffeHUijDXZqczAc1F"
    title: "Commissioned protocol/backend designs, selected capability replies and short-lived writers with the user, and established capability-oriented tracking and collection tickets"
extensions:
  ticket: tipre-tracking
  related_work: [kop-collections]
---

# Shell and terminal-state tracking

## Capability work anchors

Epic **`tipre-tracking`** connects the existing title/icon/color observer and
launcher to the following prospective capabilities. Ticket design pointers are
research inputs, not acceptance of every proposed detail.

| Ticket | Capability |
| --- | --- |
| `tipre-tracking-progress` | Retain OSC 9;4 progress independently of titles |
| `tipre-tracking-program-status` | Parse and retain OSC 7501 record trees |
| `tipre-tracking-status-lifetimes` | Apply prompt, reset and exit transitions |
| `tipre-tracking-status-responder` | Advertise support with one ordered proxy-owned reply |
| `tipre-tracking-one-shot` | Exit with the child instead of prompting for retry |
| `tipre-tracking-shell-entry` | Shell-compatible entry with explicit inner zsh |
| `tipre-tracking-entity` | Stable per-shell identity and backend-portable state |
| `tipre-tracking-publication` | Bounded off-relay publication through short-lived writers |
| `tipre-tracking-tmux-progress` | Per-pane sampled progress with explicit source scope |
| `tipre-tracking-passthrough` | Observe explicit tmux DCS-wrapped reports |
| `tipre-tracking-lightness` | Measure and optimize many-instance footprint |

Existing capability anchors remain native: `tipre-keep-icon-osc`,
`tipre-keep-color-osc`, and the completed `tipre-launcher` family. Attachment/title
policy remains `tipre-session-engine`; control-mode freshness remains
`tipre-tmux-control-mode`. New work links these rather than replacing them.

## Integration direction

- [Shell-local tracking with Kopacetic](shell-tracking0.gpt6a.md) — verified
  current capture inventory and Atuin lineage; proposed shell entry, entity
  lifetime, publication boundary, backend choice, and history ownership.

## Protocol and multiplexer evidence

- [OSC 9;4 and OSC 7501 brief](osc-protocols.glm53m.md) — G research on
  progress and program-status semantics, lifetimes, capability detection, and
  implementation support.
- [OSC 9;4 in tmux](tmux-osc94.glm53fx.md) — FX source trace through parsing,
  retained pane state, formats, terminal feature gating, and client propagation.

## Kopacetic handoff

- [Shell-writer consumer contract](file:///home/rektide/src/kopacetic/doc/consumer/term-preserver.md)
  — backend-neutral primary keys and incremental writes, Caminus-first direction,
  independent-process qualification, and the relationship to earlier collection
  work. Kopacetic owns the reusable contract; term-preserver owns protocol state.
- [Caminus collection assessment](caminus-collections.glm53fx.md) — FX trace of
  keyed directory isolation, actual legacy write behavior, and process-level
  qualification gaps; separates unwired atomic helpers from the live serializer.
- [Turso process/handle ownership assessment](turso-multiprocess.glm53m.md) — G
  pinned-driver source trace and isolated OS-process experiments. The user's
  intended shape is short-lived open/write/close publications; this is distinct
  from every proxy retaining an open database.

## Earlier foundations

- [Original PTY proxy](../../doc/design/2026-06-08-title-proxy.md) — relay and
  observer origin, including the Atuin reference.
- [Child-session orchestration](../../doc/design/2026-06-08-child-session-orchestration.md)
  — lifecycle and resource ownership reused by the tracking proposal.
- [Session launcher](../../doc/design/2026-06-22-session-launcher.md) — the
  connection-oriented use case alongside the proposed shell-local use case.
