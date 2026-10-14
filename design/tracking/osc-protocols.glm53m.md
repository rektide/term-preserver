---
type: Brief
title: OSC 9;4 (ConEmu progress) and OSC 7501 (Program Status Protocol)
description: Specs, provenance, state semantics, support, interactions, and proxy parsing recommendations for the two OSCs relevant to term-preserver shell state tracking.
resource: /design/tracking/osc-protocols.glm53m.md
status: draft
author: model:zai-coding-plan/glm-5.3#max
date: 2026-10-14
sources:
  - resource: "opencode://ses_ec8438a0cffekFPbiWgK70TqvU"
    title: "Commissioned research on OSC 9;4 and OSC 7501: read primary specs and vendor sources, reconstructed semantics, surveyed support, drafted proxy recommendations"
  - resource: "https://www.superlogical.com/rex/docs/build/program-status"
    title: "Program Status Protocol (OSC 7501) spec, revision 0.4 (2026-10-10), read 2026-10-14"
  - resource: "https://mitchellh.com/writing/program-status-osc7501"
    title: "Announcement post with provenance and implementation status, read 2026-10-14"
  - resource: "https://conemu.github.io/en/AnsiEscapeCodes.html#ConEmu_specific_OSC"
    title: "Origin specification of OSC 9;4, read 2026-10-14"
  - resource: "https://learn.microsoft.com/en-us/windows/terminal/tutorials/progress-bar-sequences"
    title: "Windows Terminal progress tutorial and 1.6+ requirement, read 2026-10-14"
  - resource: "https://ghostty.org/docs/vt/osc/conemu"
    title: "Ghostty OSC 9;4 semantics, staleness timeout, OSC 9 collision, read 2026-10-14"
extensions:
  related_work:
    - /design/tracking/README.md
    - /design/tracking/shell-tracking0.gpt6a.md
    - /design/tracking/tmux-osc94.glm53fx.md
  scope: "Design research only; no implementation."
---

# OSC 9;4 and OSC 7501 for proxy state tracking

Two child→terminal OSC sequences are candidates for term-preserver's observed
state beyond the existing title/icon/color slots:

- **OSC 9;4** — ConEmu's progress state (2012-era Windows origin, now a
  cross-platform de-facto standard).
- **OSC 7501** — the Program Status Protocol, a new (2026-09/10) generic
  state-reporting protocol from Mitchell Hashimoto, born from Rex/Superlogical
  and Ghostty work, aimed squarely at "what is this shell/program doing right
  now".

Both are reports a program writes into the PTY stream; a proxy already sees
every byte, so observing them is a parser extension, not an architectural
change. tmux-side propagation of 9;4 is covered by the companion wave
[tmux-osc94](tmux-osc94.glm53fx.md) and deliberately not re-traced here.

## OSC 9;4 — ConEmu progress

### Provenance and spec

- Origin: ConEmu's ANSI X3.64 extension set, documented on
  [conemu.github.io](https://conemu.github.io/en/AnsiEscapeCodes.html#ConEmu_specific_OSC):
  `ESC ] 9 ; 4 ; st ; pr ST` — "Set progress state on Windows 7 taskbar and
  ConEmu title."
- Carried forward by Windows Terminal (PR
  [microsoft/terminal#8055](https://github.com/microsoft/terminal/pull/8055),
  merged 2020-11-18; shipped in Windows Terminal **1.6+** per the
  [MS tutorial](https://learn.microsoft.com/en-us/windows/terminal/tutorials/progress-bar-sequences)),
  then by mintty, kitty, and Ghostty.
- It is a vendor convention with no standards-body home; the terminal-wg
  specifications repo has no proposal for it (checked 2026-10-14).

### Payload and state transitions

`ESC ] 9 ; 4 ; st [; pr] (BEL | ESC \)`

Per the ConEmu and Ghostty documents:

| st | Meaning | pr |
|----|---------|----|
| 0 | remove/inactive — clears progress | ignored |
| 1 | in progress — determinate value | 0–100 |
| 2 | error | optional; keeps last value if absent |
| 3 | indeterminate | ignored/absent |
| 4 | paused | optional; keeps last value if absent |

- Trailing/missing `pr` is legal: ConEmu allows omitting it for st 0/2/3/4;
  Windows Terminal initially mishandled that (issue
  [#9960](https://github.com/microsoft/terminal/issues/9960), fixed by PR
  [#10024](https://github.com/microsoft/terminal/pull/10024) "Allow trailing
  semicolon when parsing OSC 9;4", merged 2021-05-05). A parser must accept
  `9;4;2` and `9;4;2;` alike.
- Out-of-range `st` (>4) is rejected by WT; `pr` is clamped to 100 there.
- There is exactly one progress slot per pane/terminal; each report replaces
  it. No identity (no app name), no message, no hierarchy.

### Support snapshot (read 2026-10-14)

- **ConEmu** — origin; renders in taskbar + title.
- **Windows Terminal/conhost** — taskbar + tab progress ring since 1.6
  (2021); current source enum (`DispatchTypes::TaskbarState`:
  Clear/Set/Error/Indeterminate/Paused) matches ConEmu. Taskbar shows the
  *last-focused* tab's state when several tabs report.
- **mintty** — since 3.4.2 (2020-11-04) "CSI %q or OSC 9;4 (experimental)",
  plus heuristic progress *detection* from screen contents.
- **kitty** — 0.39.0: task progress shown as percentage in the tab title via
  `tab_title_template`; 0.40.1 (2025-03-18): macOS dock-icon progress;
  0.47.0 (2026-05-19): in-window progress bar "using the OSC 9;4 escape
  sequence", gated by the `progress_bar` option.
- **Ghostty** — implements per the table above, drawn as a bar on each split;
  per the spec note below, resets state after a hard-coded ~15 s staleness
  timeout and recommends emitters refresh ~1/s as keep-alive.
- **foot** — does not implement 9;4; since 1.20.0 it explicitly *ignores*
  OSC-9 bodies starting with `<number>;` so ConEmu sequences aren't mistaken
  for iTerm2-style OSC 9 notifications.
- **WezTerm** — no OSC 9;4 support documented in its escape-sequence docs
  (checked 2026-10-14).
- **tmux** — see the companion wave; do not rely on second-hand claims here.
- **iTerm2** — OSC 9 itself is iTerm2's notification sequence; emulators must
  disambiguate 9;n from bare OSC 9 (Ghostty sniffs the sub-ID). This
  collision is called out both by Ghostty and by the OSC 7501 spec ("has
  caused garbled output in terminals that know only one of the two").

### Emitters (existence proof that it's not Windows-only)

- winget — PR [winget-cli#883](https://github.com/microsoft/winget-cli/pull/883)
  (merged 2021-04-22) emits 9;4 alongside its spinner, including
  indeterminate state.
- PowerShell — `$PSStyle.Progress.UseOSCIndicator` (PR
  [PowerShell#14927](https://github.com/PowerShell/PowerShell/pull/14927),
  merged 2021-03).
- Trivial shell snippets in the MS tutorial (`echo -ne "\033]9;4;1;50\a"`).

### Unresolved differences

- **st=4 is the real fork**: ConEmu says *paused*; Windows Terminal's own
  tutorial says *Warning*; WT's shipped code enum says *Paused*; Ghostty says
  *paused*. Treat st=4 as "paused/warning-ish non-normal terminal state" and
  record the raw number.
- **Staleness is unspecified**: no sequence-level keep-alive or clear-on-exit;
  each terminal invents policy (Ghostty 15 s timeout; WT last-focused-tab;
  kitty per-window). A tracker must timestamp every report and not assume a
  terminal cleared it.

## OSC 7501 — Program Status Protocol

### Provenance

- Author: Mitchell Hashimoto; emerged from his work on Superlogical's
  [Rex](https://www.superlogical.com/rex/docs/build/program-status) terminal
  and Ghostty; announced 2026-10-06 in
  ["A Terminal Protocol for Program Status (OSC 7501)"](https://mitchellh.com/writing/program-status-osc7501).
  HN thread: [49984159](https://news.ycombinator.com/item?id=49984159)
  (186 points).
- Revision history in the spec: 0.1 (2026-09-28) → 0.2 (2026-10-06) → 0.3
  (2026-10-07) → **0.4 (2026-10-10)**, the revision summarized here. The
  spec is presented as a generic terminal protocol with no Superlogical
  product language; RFC 2119 keywords throughout.
- The number 7501 is unallocated space chosen by the author; there is no
  registry entry (terminal-wg has no proposal for it).

### Syntax and payload

```
OSC 7501 ; pairs ST        report
OSC 7501 ; ?     ST        feature-detection query
pairs := pair(":" pair)* ;  pair := key "=" value
key   := [a-z]+ ; value := [A-Za-z0-9_.,+/=-]*
```

- `;` and `:` are structurally excluded from values, so nothing needs
  escaping; free text (`msg`, `title`) is **base64 of UTF-8**, padding
  optional, decoded text must contain no control characters.
- Malformed pairs are skipped; unknown keys are ignored (this is the
  extension mechanism); repeated key → last wins; report over a limit or
  failing base64/control-char validation is **discarded whole**.

### Records, states, lifetimes

The terminal keeps a record set, one per `id` (`id` is a `/`-separated path,
segments `[A-Za-z0-9_.+-]{1,32}`, max 128 bytes and 8 levels; absent id →
root record). Each report **replaces its record completely** — a program
repeats `app`/`title`/`msg` in every report or loses them. `/` gives
parent/child relations used only for clearing and `app` inheritance.

`state` is the only required key:

| state | meaning | lifetime |
|-------|---------|----------|
| idle | at rest, awaiting user | until replaced/cleared |
| working | running; optional `progress` 0–100 | dropped at process exit or next shell prompt |
| done | finished; result unseen by user | **survives** process exit and prompt |
| blocked | needs user; `kind` says what, `msg` why; optional progress | same as working |
| error | failed and stopped | same as done |
| clear | not a state — removes the addressed record and everything beneath it; no id → removes all | — |

Other keys: `kind` (permission | question | auth, blocked-only),
`progress` (0–100, absent = indeterminate), `app` (stable program id,
inherited from nearest ancestor), `title`, `msg`.

**Lifetimes are the load-bearing part for a proxy**: there is no heartbeat;
terminals MUST drop `working`/`blocked` records when the process attached to
the terminal exits or when a new shell prompt begins (**OSC 133 A**), and MAY
drop `idle`; `done`/`error` survive both. A full reset (RIS) removes every
record; DECSTR does not. So consuming 7501 correctly couples the observer to
process-exit events and to the shell-integration OSC 133 stream, not just to
7501 bytes.

### Feature detection

Program may send `OSC 7501 ; ? ST`; a supporting terminal MUST reply
`OSC 7501 ; ? : states=... : kinds=... ST` (states list excludes `clear`;
omitted lists mean exactly the five states / three kinds, fixed forever).
Programs that can't read replies may skip detection and rely on terminals
ignoring unknown OSCs. Recommended probe: send `CSI c` right after the query;
if the DA reply comes back first, 7501 is unsupported. The only bytes a
terminal ever writes back are this reply — nothing from records is readable
back out (a deliberate privacy/security property).

### Limits (hard caps; violating report discarded whole)

4096 bytes whole sequence; 16-byte keys; `msg` 2732 enc / 2048 dec; `title`
256/192; `app` 32; `id` 128; ≥64 records per terminal, MAY cap at 256 with
LRU eviction. Security notes: no markup in msg/title, disarm bidi/invisible
formatting, label anything shown with its source terminal, rate-limit
out-of-band effects (notifications/sounds).

### Relationship to neighbors (per the spec itself)

- **OSC 9;4**: says busy/percent/error only; terminals MAY map 9;4 onto the
  root record but SHOULD stop mapping once a real 7501 report arrives (until
  RIS), because a mapped 9;4 would otherwise wipe `kind`/`msg`.
- **OSC 9 / 99 / 777 notifications**: one-time events, no current-state
  residue, imply interruption.
- **OSC 133**: prompt/command markers from the shell; says a command
  started/ended, not what it's doing; 7501 lifetimes hook into its `A` mark.
- **OSC 0/2 titles**: what programs overload today with glyphs/spinners —
  the problem being replaced.
- **OSC 21337 (iTerm2 session status) and OSC 9999 (Orca)**: prior art the
  spec characterizes as presentation-heavy or JSON-requiring.

### Support and adoption (as of 2026-10-14 — one week old)

Terminals/implementations: **Rex** (native, surfaces in session picker, tab
headers, Lua `program_status_changed` events); **libghostty/Ghostty** (per
the blog "one implementation in libghostty"; Ghostty's public VT docs do not
yet list it — checked 2026-10-14; RoyalTerminal PR #121 bundles updated
Ghostty with "native/managed OSC 7501 support"); **BubbleTea v2.1** (per HN
participant nimblemarkets/neomantra). Open (unmerged) requests in a wave of
terminals: dormouse #1079, Bobcat #67, okena #220, luvus #498,
con-terminal #443.

Emitters/pledges (all open issues/PRs unless noted): Pi terminal (documented
emitter — pi.dev docs map working/blocked/done/error/idle), opencode TUI PR
("hierarchical OSC 7501"), Qwen Code PR, tqdm #1873, Neovim #42276,
gemini-cli, copilot-cli, kimi-code, MiMo-Code, goose, stakpak, mastra,
kimchi, nextflow. GitHub-wide search for `"OSC 7501"` returns thousands of
items dated 2026-10-06..13 — adoption velocity is exceptional, but virtually
all non-author implementation work is *pending*, not shipped.

### Unresolved differences and criticism

- **Competing/adjacent standards**: [UAPI.15 OSC 3008 "Hierarchical Context
  Signalling"](https://uapi-group.org/specifications/specs/osc_context/)
  (start=/end= context trees with metadata; something in Ubuntu 26 emits it —
  WindTerm #3653 shows raw `3008` sequences leaking to non-supporting
  terminals over SSH). HN argued for extending 3008 instead; its syntax also
  drew criticism (JdeBP: "ASCII byte range > 0x7f" in the grammar). SIGINFO/^T
  (BSD pull model) and the terminal bell are the classic incumbents.
- **`kind` taxonomy** (permission/question/auth) is seen as AI-agent-shaped
  and likely to grow (HN: paaloeye, cpuguy83); unknown kinds are already
  treated as absent, so consumers must tolerate drift.
- **OSC vs APC framing**: ECMA-48 purists say this belongs in APC; the
  author's answer is that APC support is worse in the wild (Terminal.app
  dumps APC to the screen) and OSC multiplexing-by-first-number is the
  de-facto reality.
- **No registry**: the number space is squatting-by-convention; rev 0.3→0.4
  already changed feature-detection replies (now lists states/kinds), and
  0.1→0.3 changed absent-progress to mean indeterminate. Track `states=`/`kinds=`
  capability strings rather than assuming the fixed five/three.
- **Relying on graceful degradation** assumes every terminal ignores unknown
  OSCs; real terminals (and WindTerm above) sometimes don't.

## What this means for term-preserver

### Fit with the proxy's observation model

Both sequences are pure observations for the existing PTY→stdout observer;
nothing about the data plane changes. The differences from title tracking:

1. **9;4 is one mutable scalar-ish slot** (`{state, progress, timestamp}`)
   per shell — exactly the shape of the existing title/icon/color slots.
2. **7501 is a small record set with protocol-defined GC**. To hold "latest
   state" correctly the proxy must also watch **OSC 133 A** (new prompt) and
   child exit, because the spec makes those the destruction events for
   working/blocked. If term-preserver only passively stores reports it will
   retain stale "working" states a real terminal would have dropped.
3. **No heartbeat in either protocol** — a tracking recommendation (ours,
   not a normative protocol rule): timestamp every capture so staleness is a
   consumer decision (Ghostty chose 15 s for 9;4). The specs themselves
   mandate no timestamps and no re-send.

### Parsing recommendations (src/osc.rs)

- The streaming state machine already terminates OSC bodies on BEL/ESC \.
  Note the limits are *different kinds of bound*: the existing parser cap
  applies to captured **data** after `ESC ]`, while the 7501 spec caps the
  **whole sequence** — OSC introducer through terminator, framing bytes
  included. Equal numbers do not make them equivalent: a spec-conformant
  check must account for framing bytes itself (introducer `ESC ]`, code
  `7501`, separators, and the two-byte ST or one-byte BEL) rather than lean
  on the data cap. Extend `OscCapture` with `Progress { state, progress }`
  and `ProgramStatus { … }` variants; route on the leading code before `;`
  (the parser already dispatches `0/1/2/4/10/11/12`).
- 9;4: parse `st` and optional `pr`; keep prior `progress` when `pr` is
  absent under st 2/4 (ConEmu/WT#10024 semantics); reject st>4; clamp pr to
  100; store raw `st` rather than a semantic enum, given the paused/warning
  fork. Accept BEL and ST terminators and missing trailing `pr`/`;`.
- 7501: split pairs on `:` — note `;` never appears inside a 7501 body by
  construction, so the existing "OSC data until terminator" handling needs no
  change. Validate *then* apply (spec: check every pair before touching
  stored records; discard whole on any limit/base64/control-char failure).
  base64 decode for `msg`/`title` with control-character refusal; id-grammar
  check before addressing a record; last-wins on duplicate keys; ignore
  unknown keys. Record per-id, replacing wholly; implement `clear`
  recursively.
- OSC 9 disambiguation: bare `OSC 9 ; body` without a leading `number;` is
  iTerm2 notification territory (foot ignores 9;n entirely; Ghostty sniffs
  the sub-ID). If the proxy ever reacts to OSC 9, it must not treat `9;4` as
  a notification or vice versa.
- Stripping/re-emit: title filters should leave both sequences alone (they
  aren't titles). For `--reemit-title` on child exit, re-emitting captured
  9;4 as-is can resurrect a stale spinner, and re-emitting 7501 `working`
  after the process died contradicts the spec's own lifetime rule — either
  suppress stateful slots from re-emit or re-emit only `done`/`error`
   deliberately.
- Feature detection: replying to `OSC 7501 ; ?` is an *optional design role*
  for the proxy, not an obligation the spec attaches to anything. The spec
  leaves presentation entirely undefined — a replier promises only the
  states/kinds grammar, and consuming reports into a registry/status store is
  itself the useful behavior; no translation into window titles or other
  outer-terminal presentation is implied or required. The real design need is
  an explicit **single-responder strategy**: either intercept the query and
  answer on the terminal's behalf (one reply owner, serialized with the stdin
  relay), or pass it through untouched and stay silent — never both, or the
  child can receive duplicate/conflicting replies. The corpus's recorded
  human selection (proxy-owned capability replies in tracking mode) lives in
  [shell-tracking0](shell-tracking0.gpt6a.md); the ordering constraint that
  matters there is that a proxy reply must precede any following primary-DA
  probe an emitter uses to detect non-support.

### Fit with the Kopacetic entity model

The commissioned direction — lightweight login-shell proxy, one entity per
shell retaining *latest* state, history owned by Kopacetic — matches the
protocols unusually well:

- 7501's model *is* latest-state-wins: records replace wholly, lifetimes are
  edge-triggered (report, prompt, exit), and the spec explicitly forbids
  requiring heartbeats. Publish an entity snapshot per report and per
  lifetime edge (working/blocked dropped at prompt/exit; done/error persist)
  and the entity mirrors what any 7501-aware terminal would show.
- 9;4 maps to a single scalar field plus timestamp on the same entity.
- Neither protocol carries history; that cleanly stays Kopacetic's
  responsibility, per the existing division in
  [shell-tracking0](shell-tracking0.gpt6a.md).
- Transport transparency differs: ssh and mosh carry both sequences as
  opaque bytes, but **tmux is not a byte pipe for them** — in the inspected
  revision tmux parses OSC and *consumes* unknown sequences such as 7501
  (they do not reach the outer terminal), and it re-emits the selected
  pane's 9;4 only conditionally. Whether the proxy sees these sequences at
  all therefore depends on whether it sits outside or inside tmux; placement
  is a coverage decision, not a detail. The tmux-side evidence and client
  propagation rules are traced in the companion wave
  [tmux-osc94](tmux-osc94.glm53fx.md); do not infer them from this brief.

## References (read 2026-10-14 unless noted)

Spec-sourcing note: the primary spec (rev 0.4) was read directly both by this
brief's author and, independently, by the commissioning session that
requested the corrections recorded here.

Primary:

- Rex spec, rev 0.4: <https://www.superlogical.com/rex/docs/build/program-status>
- Blogpost: <https://mitchellh.com/writing/program-status-osc7501>
- HN discussion: <https://news.ycombinator.com/item?id=49984159>
- ConEmu OSC table: <https://conemu.github.io/en/AnsiEscapeCodes.html#ConEmu_specific_OSC>
- MS tutorial: <https://learn.microsoft.com/en-us/windows/terminal/tutorials/progress-bar-sequences>
- Ghostty VT docs (OSC 9;n): <https://ghostty.org/docs/vt/osc/conemu>

Vendor sources:

- microsoft/terminal PR #8055, issue #9960, PR #10024; current
  `adaptDispatch.cpp` + `DispatchTypes.hpp` (TaskbarState enum) from `main`
- mintty releases 3.4.2→3.6.4 notes (GitHub releases API)
- foot CHANGELOG 1.20.0 (codeberg)
- kitty changelog 0.39.0 / 0.40.1 / 0.47.0 (sw.kovidgoyal.net)
- wezterm `docs/escape-sequences.md` (absence of 9;4)
- winget-cli PR #883; PowerShell PR #14927 / issue #14208
- UAPI.15 OSC 3008: <https://uapi-group.org/specifications/specs/osc_context/>
- WindTerm #3653 (OSC 3008 leak on Ubuntu 26)
- RoyalTerminal PR #121 (Ghostty bundling); pi.dev terminal-setup docs

Adoption survey: GitHub search API, `"OSC 7501"`, 2026-10-14 (issues/PRs
cited inline; nearly all open at read time).
