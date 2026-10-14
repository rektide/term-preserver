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
    title: "Commissioned FX/G protocol and backend research, developed the shell design, and handed write-first collection requirements to Kopacetic"
---

# Shell and terminal-state tracking

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

## Earlier foundations

- [Original PTY proxy](../../doc/design/2026-06-08-title-proxy.md) — relay and
  observer origin, including the Atuin reference.
- [Child-session orchestration](../../doc/design/2026-06-08-child-session-orchestration.md)
  — lifecycle and resource ownership reused by the tracking proposal.
- [Session launcher](../../doc/design/2026-06-22-session-launcher.md) — the
  connection-oriented use case alongside the proposed shell-local use case.
