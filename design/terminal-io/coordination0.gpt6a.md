---
type: Design
title: Shared terminal traffic observation and coordination
description: Proposed common I/O capabilities for status replies, notification feedback, and externally scriptable colors.
resource: /design/terminal-io/coordination0.gpt6a.md
status: draft
author: model:openai/gpt-6-astra
date: 2026-10-14
sources:
  - resource: "opencode://ses_ec84404aaffeHUijDXZqczAc1F"
    title: "Review of notification/color drafts and capability tickets; factor shared I/O ownership instead of coupling unrelated protocols"
  - resource: /src/session.rs
    title: Current stdin-owned PTY writer and independently owned stdout relay
  - resource: /design/tracking/shell-tracking0.gpt6a.md
    title: Human-selected OSC 7501 capability responder and lightweight shell proxy
  - resource: /design/notifications/notifications0.glm53m.md
    title: G draft establishes upstream notification reports as a new observation point
  - resource: /design/colors/colors0.glm53m.md
    title: G draft proposes external writes and queries; review identified query-source ownership requirements
extensions:
  ticket: tipre-terminal-io
  related_work: [tipre-tracking-status-responder, tipre-notifications, tipre-colors]
verification:
  method: Source and design review only; no implementation or traffic tests
---

# Shared terminal traffic capabilities

**Proposed design; ticket linkage is not acceptance.** The concrete protocol
choices remain with their capability tickets. This proposal gives them shared
traffic ownership without making one protocol a prerequisite for another.

## Why a shared surface

The existing proxy has one thread writing stdin bytes to the child PTY and
another relaying child output to stdout. Three commissioned features cross those
boundaries:

- OSC 7501 responds **toward the child** and suppresses its intercepted query.
- Kitty notification feedback arrives **from the outer terminal**, on stdin.
- External color commands write **toward the outer terminal** and may await a
  response that belongs to the script, not to the child shell.

Implement these capabilities once. OSC 99 must not wait for the entire OSC 7501
state machine merely to observe input; color querying must not wait for complete
notification support merely to receive terminal replies.

## Capability decomposition

| Ticket | Surface |
| --- | --- |
| `tipre-terminal-io-input` | Observe terminal-to-child traffic and serialize child-directed reply injection |
| `tipre-terminal-io-output` | Order child output and externally injected terminal commands |
| `tipre-terminal-io-queries` | Track query origin, response ownership, ambiguity and timeout |

Keep the existing blocking relay model unless measurements justify a change.
This is an ownership seam, not a requirement for a generic async runtime or
another persistent process. Measure it under `tipre-tracking-lightness`.

## Input and output invariants

Ordinary input passes through unchanged. An observer can annotate a recognized
reply, but observation alone does not claim or remove bytes. Only an explicit
query owner may consume a matched response. Synthetic replies to the child are
written as complete ordered units through the same input owner as user bytes.

Likewise, external terminal commands join one ordered output path. They must not
interleave into a partially emitted child control sequence. Define the insertion
boundary, bounded buffering, and malformed-stream behavior; a mutex around one
arbitrary read chunk is not a complete framing policy. Record dispatched commands
through the state reducer without feeding emitted bytes around an observation
loop. Successful byte emission is not proof the terminal applied a color.

For the selected OSC 7501 responder, consume the child's capability query and
send one child-directed reply before forwarding a subsequent DA probe that could
otherwise win the emitter's detection race. Forward actual reports normally.

## Query ownership

Legacy and OSC 21 color replies lack request IDs. Serializing only proxy-origin
queries cannot disambiguate a racing child query for the same color.

The query capability must therefore either coordinate all supported visible
query sources or state the unsupported/ambiguous situations. A possible bounded
policy defers proxy queries while child requests are outstanding, tracks expected
reply shapes/counts, and serializes further requests. This is a proposal requiring
fixtures, especially for timeouts and delayed replies; shape matching by itself
is not proof of ownership.

- Child-origin requests keep their replies routed to the child.
- Proxy-origin requests return their matched replies to the controlling script;
  those bytes must not become unsolicited shell input.
- Ambiguous or late responses are handled by an explicit policy that favors
  preserving child traffic over fabricating a successful external query.
- Unsupported requests return unknown/unsupported, not a cached value labeled
  as a fresh terminal response. No color reply is synthesized from recorded state.

Label a queried result by its upstream terminal scope and observation time. With
tmux or another intermediary, a response can describe that responding layer;
it is not universal proof of the final physical terminal's configuration.

Capability answering is protocol-specific. Recording OSC 7501 is the selected
service being advertised; recording notification requests does not imply an OS
notification-delivery service or authoritative knowledge of the terminal's live
notification set.

## Verification surface

Use a scripted outer terminal and real child PTY to cover fragmented frames,
simultaneous user input/injected replies, child and proxy queries for the same
slot, query-plus-DA ordering, multiple replies, delayed replies after timeout,
and external color writes during split child control sequences. Ordinary byte
forwarding and terminal cleanup remain observable assertions throughout.

Related designs: [tracking](../tracking/shell-tracking0.gpt6a.md),
[notifications](../notifications/notifications0.glm53m.md),
[colors](../colors/colors0.glm53m.md). The [capability index](../tracking/README.md)
connects the native work anchors.
