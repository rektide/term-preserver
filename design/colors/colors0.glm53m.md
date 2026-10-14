---
type: Design
title: Readable and writable terminal color state (legacy OSC 0-12/104-112, kitty OSC 21)
description: PROPOSED design for accurate color state observation, external read/write/query control, and the kitty OSC 21 color-control protocol, on top of the delivered legacy OSC capture.
resource: /design/colors/colors0.glm53m.md
status: draft
author: model:zai-coding-plan/glm-5.3#max
date: 2026-10-14
sources:
  - resource: "opencode://ses_ec7f4e3bcffeQTeR4NqyDYJxS7"
    title: "Commissioned colors design: verified legacy + kitty OSC 21 semantics against xterm ctlseqs and kitty docs/source, drafted state model, control surface, and tickets"
  - resource: "https://vtdn.dev/docs/osc/kitty-osc21"
    title: "vtdn kitty OSC 21 page (AI-generated, experimental) — read 2026-10-14; pair separator corrected against the primary spec and kitty source"
  - resource: "https://sw.kovidgoyal.net/kitty/color-stack/"
    title: "Primary kitty color-control spec (OSC 21, push/pop 30001/30101, xterm interop), read 2026-10-14"
  - resource: "https://raw.githubusercontent.com/kovidgoyal/kitty/master/kitty/window.py"
    title: "kitty color_control source; confirms semicolon-split pairs, unknown= base64 replies, bare-key reset, empty-value dynamic, printable-ASCII sanitization (read 2026-10-14)"
  - resource: "https://invisible-island.net/xterm/ctlseqs/ctlseqs.txt"
    title: "xterm ctlseqs: OSC 4/5/6/10-19 set-or-query semantics, '?' same-form replies, reply terminator mirroring, 104/105/110-119 resets (read 2026-10-14)"
  - resource: /src/osc.rs
    title: "Current parser: capture variants, first_segment behavior, palette chaining, reset handling"
  - resource: /src/color.rs
    title: "ColorSpec raw + normalized RGB representation"
  - resource: /src/session.rs
    title: "ObservedSlots latest-value model; Option semantics conflate reset and never-observed"
  - resource: /src/prompt.rs
    title: "reemit_sequences reconstruction of captured color state on child exit"
  - resource: /design/notifications/notifications0.glm53m.md
    title: "Companion wave: stdin-side observation point and responder strategy, shared machinery"
  - resource: /design/tracking/shell-tracking0.gpt6a.md
    title: "Prior shell-tracking design; publication boundary and entity direction (parent-owned)"
  - resource: /design/tracking/tmux-osc94.glm53fx.md
    title: "Prior tmux OSC trace; multiplexer consumption caveats"
verification:
  method: "Spec reading (xterm ctlseqs, kitty primary docs + parser source; vtdn cross-checked), code reading of the current parser/session/prompt. Nothing implemented or runtime-tested; legacy chained-parameter divergence identified by spec reading only."
extensions:
  related_work:
    - /design/tracking/README.md
    - /design/notifications/notifications0.glm53m.md
  tickets: tipre-colors
  scope: "Design research only; no implementation."
---

# Readable and writable terminal color state

**Status: PROPOSED design.** Ticket links point here; they do not signify
acceptance. The delivered capture capability (closed `tipre-keep-color-osc`,
`tipre-keep-icon-osc`) is the foundation — this design extends it, and does
not re-scope what already works. Publication and shell entry remain
parent-owned in
[shell-tracking0](../tracking/shell-tracking0.gpt6a.md).

## Situation and goal

Today the proxy records, as latest-value slots, what the child *wrote*:
window/icon titles (OSC 0/1/2), palette and special colors (OSC 4/5),
fg/bg/cursor (OSC 10/11/12), and the resets (OSC 104/105/110/111/112),
re-emitting all of it on exit under `--reemit-title`. The user goal is
sharper: **readable current color state, eventually writable, scriptable
from outside the child** — think `my-theme set catppuccin` while a session
runs, and `term-preserver color get` answering truthfully.

Two protocol surfaces are in play: the legacy xterm set/query family the
project already parses, and kitty's OSC 21 color-control protocol, which is
the "thinking bigger" direction — named keys, batched set+query, explicit
dynamic and reset semantics, and an answer for colors xterm cannot express.

## Legacy family — what the specs actually say (verified)

From [xterm ctlseqs], the bits that shape this design:

- **Set-or-query duality.** For OSC 4, 5, and 10–19, a `?` in place of a
  color spec makes the sequence a query: xterm replies with *the same-form
  sequence that would set the color* (`OSC 10 ; rgb:… ST`), using **the same
  terminator the query used** (BEL or ST). A multi-pair OSC 4 query can
  produce **multiple replies**. Today's parser would record a query
  (`10;?`) as a capture with spec `?` — a latent bug class for state
  accuracy once queries appear in real streams (shell integrations and
  tools like `kitten query-terminal` send them routinely).
- **Chained dynamic-color parameters.** For OSC 10–19, "each successive
  parameter changes the next color in the list": `OSC 10 ; red ; blue` sets
  foreground *and* background. The current parser takes only the first
  segment (`first_segment`, osc.rs) — a spec divergence to fix for state
  accuracy, independent of any new capability.
- **Special colors.** OSC 5 sets indexed "special" colors (bold, underline,
  blink, reverse, italic…); OSC 6/106 enable/disable them by mode — the
  project parses 5 and 105, not 6/106.
- **Resets.** OSC 104/105 accept any number of indices (none = whole
  table); OSC 110–119 reset the dynamic colors to their **X-resource
  defaults**. Reset targets are terminal-side resources the proxy cannot
  know the values of — resets are observations of intent, not of resulting
  color.

## kitty OSC 21 — color control (authoritative)

Primary spec: [kitty color-stack], parser verified in kitty's
`window.py::color_control` (`value.split(';')` — **pairs are semicolon-
separated**; the vtdn page's colon syntax is wrong).

```
OSC 21 ; key=value ; key=value ; … (BEL | ESC \)
key   := 0-255 (ANSI table) | foreground | background | selection_background
       | selection_foreground | cursor | cursor_text | visual_bell
       | transparent_background_color1..7
value:= color-spec | "?" (query) | "" (dynamic) ; bare key (no "=") = reset
```

- **Query:** `key=?` → terminal replies with the same escape code, `?`
  replaced by the color (`rgb:rr/gg/bb`, always this form). A color with no
  defined value — reverse-video selections, dynamic cursor — replies with
  an **empty value** (`cursor=`). Unknown keys are echoed back as
  `unknown=<base64-of-key>` (unpadded, so the reply can't be abused to emit
  arbitrary text), one per unknown key, in order.
- **Set:** value is a color spec (X11 `rgb:`/`#`/`rgbi:` + alpha `@0.5` +
  color names; same family `ColorSpec` already parses). **Empty value**
  (`cursor=`) means *dynamic* (e.g. cursor takes the text color). **Bare
  key** (`background`) means *reset to default*. Set and query can be
  combined in one sequence (`foreground=white ; foreground=?`).
- **Sanitization:** kitty strips non-printable-ASCII before echoing — the
  query channel is constrained by construction.
- **Color stack:** kitty push/pop `OSC 30001` / `30101` (and xterm's later
  XTPUSHCOLORS/XTPOPCOLORS `CSI … # P/Q` + XTREPORTCOLORS, which kitty also
  accepts) save/restore the whole table + dynamic colors. *Not proposed
  here* — flagged as future scope because it changes what "current state"
  means (a restore can silently rewrite every slot the proxy recorded).
- Minor doc/source divergence, recorded not resolved: the docs table says
  `transparent_background_color1..7`, kitty's parser accepts indices 1..8.
- Support reality (vtdn matrix, not independently verified): kitty ≥0.36 and
  Ghostty implement OSC 21; nothing else surveyed does. Legacy 10/11/12/4
  queries have far wider support. A proxy that speaks both is useful in
  more places than either alone.

## Conceptual distinctions

**Recorded state vs applied terminal state.** `ObservedSlots` is a cache of
*child writes*. The terminal's actual colors additionally include its
configured defaults, writes from before the proxy attached, other tabs'
tooling, and multiplexer policy (tmux rewrites/consumes OSC — see
[tmux-osc94]). Two honest read levels follow: *recorded* (what the child
asked for under this proxy — always available, exact) and *queried* (what
the terminal says now — needs a roundtrip, may say `dynamic`/empty).
Publishing or scripting must label which level it returns; a read that
silently substitutes one for the other lies by omission.

**Write authority.** Today only the child writes (observed). The goal adds
an **external writer**: a control surface that injects OSC toward the outer
terminal *and* records into the same state, so recorded state stays coherent
with what was applied. Writes should prefer OSC 21 when the outer terminal
supports it (named keys, one sequence) and fall back to legacy OSC 4/10/11/12
— capability detection is a query-roundtrip concern (below). Injected bytes
join the PTY→stdout path at a defined point (the relay's write side), never
the parser's input, so observation and injection cannot loop.

**Read authority and query roundtrips.** Only the terminal can answer
"what color is it really". Replies arrive on the **stdin side** of the
proxy (terminal → child), which today is relayed unexamined — the same new
observation point the notifications design needs. Constraints the query
design must respect: replies are **not request-correlated** (no ids in
legacy or 21), so queries must be **single-flight** (serialize, match by
shape, timeout to "unknown"); the reply terminator mirrors the query's; a
racing child can interleave its own reads. The proxy should **forward and
sniff, never answer from recorded state** — the terminal is the authority,
and the recorded overlay does not know resource defaults.

**Reset, dynamic, unset — three different things.** Legacy resets
(104/110-…) mean "terminal, restore your default" — the proxy records the
*intent* and loses the previous value. OSC 21 adds an explicit *dynamic*
notion distinct from reset, and distinguishes both from set. The current
`Option<ColorSpec>` conflates never-observed with reset. An accurate model
needs per-slot: `Set(raw, rgb)` / `Dynamic` / `ResetToDefault` /
never-observed, with the raw spec retained alongside normalized RGB (as
`ColorSpec` already does) because `rgb:` scaling, alpha, and names do not
round-trip through RGB alone — re-emit already depends on raw specs.

**Re-emit vs write.** `--reemit-title` is invisible restore of captured
writes; external write is a new, deliberate mutation. OSC 21 sets and named
resets should join the re-emit set; a future color-stack pop would be the
cleaner restore, noted above as out of scope.

## Proposed proxy design

1. **State model** (`session.rs`): per-slot `ColorSlot` with the four-way
   state above for fg/bg/cursor, per-index palette and special entries, and
   a new named-slot map for OSC 21-only colors (selection fg/bg, cursor
   text, visual bell, transparent backgrounds). `SessionExit` snapshots the
   whole structure. Backward-compatible accessors keep existing consumers
   working.
2. **Parser** (`osc.rs`): route param 21 to a `ColorControl` capture
   (sequence of `key { Set(spec) | Dynamic | Reset | Query }` records);
   keep unknown keys verbatim in the capture for logging. Distinguish
   legacy queries: `4;c;?` / `10;?` become query captures, not state writes.
   Fix the OSC 10–19 chained-parameter divergence while touching this area.
3. **Control surface** (new, small): a Unix-socket or CLI channel
   (`term-preserver color get|set|query|watch`) — `get` returns recorded
   state (level-labeled); `set` injects (OSC 21 when detected, else legacy)
   and records; `query` performs a single-flight forward-and-sniff
   roundtrip and returns terminal-truth; `watch` streams slot changes.
   Phasing: `get` needs only the state model; `set` needs injection;
   `query` needs the stdin-side observer. Socket path via env var so
   external scripts can find the running proxy.
4. **Shared machinery**: the stdin-side OSC observer and the
   injection/serialization point are shared with the notifications work
   ([notifications0](../notifications/notifications0.glm53m.md)) and the
   7501 responder ticket (`tipre-tracking-status-responder`); build once,
   consume from both.
5. **Not in scope**: color stacks (30001/30101, XTPUSHCOLORS), OSC 6/106
   special-mode toggles, RIS/full-reset tracking, tmux-internal color
   policy. Each interacts with "current state" semantics and deserves its
   own look before any proposal.

## Tickets (pointer, not acceptance)

Epic **`tipre-colors`**: honest, externally readable — then writable, then
queryable — terminal color state.

- `tipre-colors-readable` — the four-way slot state model, legacy query
  disambiguation, chained-parameter fix, and a level-labeled read surface
  (printed snapshot + control-channel `get`).
- `tipre-colors-osc21` — observe kitty OSC 21 set/dynamic/reset/query
  captures into the state model and re-emit coverage.
- `tipre-colors-write` — external write path: control-channel `set`
  injecting OSC 21/legacy toward the outer terminal and recording.
- `tipre-colors-query` — single-flight query roundtrip over the stdin-side
  observer, capability detection for write preference, timeout policy.

## References

- Primary: [kitty color control](https://sw.kovidgoyal.net/kitty/color-stack/);
  [xterm ctlseqs](https://invisible-island.net/xterm/ctlseqs/ctlseqs.txt);
  kitty source `kitty/window.py` (OSC 21 parsing confirmed).
- vtdn page (AI-generated placeholder, corrected where load-bearing):
  [kitty OSC 21](https://vtdn.dev/docs/osc/kitty-osc21).
- In-repo: [notifications wave](../notifications/notifications0.glm53m.md)
  (shared stdin observation/responder machinery),
  [shell tracking design](../tracking/shell-tracking0.gpt6a.md)
  (publication boundary),
  [tmux OSC trace](../tracking/tmux-osc94.glm53fx.md) (multiplexer caveat).
