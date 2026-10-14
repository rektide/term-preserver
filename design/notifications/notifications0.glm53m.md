---
type: Design
title: Desktop notification observation (OSC 9, OSC 777, kitty OSC 99)
description: PROPOSED protocol brief and proxy design for observing child-issued desktop notifications, their events, lifecycles, reverse channels, and capability queries.
resource: /design/notifications/notifications0.glm53m.md
status: draft
author: model:zai-coding-plan/glm-5.3#max
date: 2026-10-14
sources:
  - resource: "opencode://ses_ec7f4e3bcffeQTeR4NqyDYJxS7"
    title: "Commissioned notification design: read vtdn + primary kitty specs, verified against kitty source, drafted proxy design and tickets; revision 2 applied review corrections (mosh transport, cap accounting, shared-I/O and delivery-evidence boundaries)"
  - resource: "https://vtdn.dev/docs/osc/osc9"
    title: "vtdn OSC 9 page (AI-generated, experimental) — syntax, ConEmu subcommands, terminal support table, read 2026-10-14"
  - resource: "https://vtdn.dev/docs/osc/osc777"
    title: "vtdn OSC 777 page (AI-generated, experimental) — urxvt notify syntax and support table, read 2026-10-14"
  - resource: "https://vtdn.dev/docs/osc/kitty-osc99"
    title: "vtdn kitty OSC 99 page (AI-generated, experimental) — read 2026-10-14; several field semantics corrected against the primary spec below"
  - resource: "https://sw.kovidgoyal.net/kitty/desktop-notifications/"
    title: "Primary kitty desktop-notifications spec (OSC 99), read 2026-10-14"
  - resource: "https://raw.githubusercontent.com/kovidgoyal/kitty/master/kitty/notifications.py"
    title: "kitty notification parser source; confirms colon-split metadata and payload/metadata separation, read 2026-10-14"
  - resource: /src/osc.rs
    title: "Current streaming OSC parser, capture routing, and byte-classification relay contract"
  - resource: /src/session.rs
    title: "ObservedSlots/ObservedState latest-value model and SessionExit snapshot boundary"
  - resource: /src/proxy.rs
    title: "Retry-loop lifecycle, reemit-title and print-gone-title wiring"
  - resource: /design/tracking/osc-protocols.glm53m.md
    title: "Prior brief: OSC 9;4 progress and OSC 7501 program status (previous wave, available context)"
  - resource: /design/tracking/shell-tracking0.gpt6a.md
    title: "Prior shell-tracking design; entity/publication direction and proxy-owned capability-reply selection (previous wave, available context)"
  - resource: /design/tracking/tmux-osc94.glm53fx.md
    title: "Prior tmux OSC trace for 9;4; tmux handling of 9/777/99 not retraced here"
verification:
  method: "Spec reading (primary kitty spec + source for OSC 99; vtdn cross-checked), code reading of the current parser/session. Nothing implemented or runtime-tested; terminal support matrices are vtdn-sourced and not independently verified."
extensions:
  related_work:
    - /design/tracking/README.md
    - /design/colors/colors0.glm53m.md
  tickets: tipre-notifications
  scope: "Design research only; no implementation."
---

# Desktop notification observation — OSC 9, OSC 777, kitty OSC 99

**Status: PROPOSED design.** Ticket links point here; they do not signify
acceptance of these choices. Parent-owned work (shell entry, Kopacetic
publication, tmux passthrough) is linked, not redesigned.

## Situation

A child program can ask the terminal to raise an OS-level desktop
notification. Three wire conventions matter today, in ascending order of
capability:

| Sequence | Shape | Origin | State left behind |
| --- | --- | --- | --- |
| OSC 9 | `ESC ] 9 ; text ST/BEL` | iTerm2 growl / ConEmu family | none — one-shot |
| OSC 777 | `ESC ] 777 ; notify ; title ; body ST/BEL` | rxvt-unicode | none — one-shot |
| OSC 99 | `ESC ] 99 ; meta ; payload ST/BEL` | kitty 0.36 (rich protocol) | id-keyed live set in the terminal |

term-preserver already relays all of these unchanged — the parser holds them
(`osc.rs` `finish_osc` returns `None` for unrecognized params) and the relay
flushes the bytes — but records nothing. The gap this design fills is
**observation**: an event record the proxy keeps, suitable for later
publication (Kopacetic entity updates) and reconnect summaries, without
changing delivery.

## Protocol briefs

### OSC 9 — bare notification

- `OSC 9 ; text (BEL | ESC \)`. Free text payload, no title/body split, no id,
  no reply, no query. Pure fire-and-forget event.
- **Collision discipline is the only subtlety.** ConEmu multiplexes
  subcommands under OSC 9: `9;4` progress (already briefed in
  [osc-protocols](../tracking/osc-protocols.glm53m.md)), `9;9` CWD, `9;12`
  mark. Terminals split on whether a leading `number;` means subcommand or
  notification text: foot ignores every `9;n` body outright; Ghostty sniffs
  the sub-Id; kitty treats bare OSC 9 as the iTerm2 legacy notification; VTE
  and Konsole implement only 9;4. An observer must key on the *first data
  segment*: numeric+`;` → ConEmu subcommand territory (only 9;4 tracked
  elsewhere), otherwise → notification text.
- vtdn support table (not independently verified): iTerm2, foot, Ghostty,
  WezTerm, kitty (legacy), contour, Windows Terminal (only subcommands).
  xterm, VTE, Konsole, mintty, Alacritty, tmux: no.

### OSC 777 — urxvt notify

- `OSC 777 ; notify ; title ; body (BEL | ESC \)`. The general urxvt form is
  `777;command;args…`; only `notify` is broadly adopted. Missing trailing
  `;body` must be tolerated (observers should record `body: Option<String>`).
- No ids, no replies, no query — same one-shot shape as OSC 9 with a body
  field. Some terminals display only when unfocused (display policy is the
  terminal's, invisible to the proxy).
- vtdn support table (not independently verified): foot 1.6+, Ghostty,
  kitty 0.23+, WezTerm, contour, Konsole, terminology, VTE (behind an opt-in
  property). Notably absent: xterm, Windows Terminal, Alacritty, mintty,
  tmux.

### kitty OSC 99 — rich notifications (authoritative)

Primary spec: the [kitty desktop-notifications page](https://sw.kovidgoyal.net/kitty/desktop-notifications/). **The vtdn page for OSC 99 is
materially wrong in several places**; everything below is from the primary
spec, with kitty's parser source (`kitty/notifications.py`,
`metadata.split(':')`) confirming the framing.

Framing — the two semicolons are mandatory even with empty metadata:

```
ESC ] 99 ; metadata ; payload (BEL | ESC \)
metadata := pair(":" pair)*          pair := key "=" value
key      := single char [a-zA-Z]
value    := word over [a-zA-Z0-9_-+.,(){}[]*&^%$#@!`~/]
```

Semantics of the keys that matter to an observer (full table in the spec):

| Key | Meaning (corrected) |
| --- | --- |
| `p` | payload type: `title` (default), `body`, `close`, `icon`, `buttons`, `?` (query), `alive` (poll). Unknown payload types must be ignored by terminals |
| `i` | notification identifier, `[a-zA-Z0-9_-+.]`, should be UUID-like; `i=0` is the backwards-compat default in responses |
| `d` | **done** flag 0/1 (default 1) — 0 holds display while chunks accumulate. *vtdn wrongly calls this a 0/1/2 delivery mode* |
| `e` | **payload is base64** UTF-8 (RFC 4648). *vtdn wrongly calls this event reporting* |
| `a` | actions on activation: comma list of `report`, `focus`, each with optional leading `-`; default `focus` |
| `c` | `1` → terminal sends a close event back |
| `o` | occasion: `always` (default), `unfocused`, `invisible`. *vtdn wrongly defaults this to unfocused* |
| `u` | urgency 0/1/2 (low/normal/critical) |
| `w` | expiry ms: `-1` OS default, `0` never, `>0` terminal-guaranteed close |
| `f`/`t` | app name / type, base64, for filtering; `t` repeatable |
| `n`/`g` | icon by name (base64, repeatable) / cached icon-data UUID |
| `s` | sound name, base64 (`system`, `silent`, `error`, `warn`, `info`, `question`) |

- **Chunking:** repeated `p=title`/`p=body` parts under one `i` concatenate;
  payload ≤ 2048 bytes raw / 4096 encoded per chunk. An observer must
  assemble parts per id until `d≠0` before treating text as complete.
- **Update/close:** a later notification with the same `i` replaces it;
  `p=close` with `i` closes it (no-op if absent). Unidentified notifications
  are never updated.
- **Terminal→application reverse channel** (these arrive on the *stdin side*
  of a proxy, terminal output toward the child):
  - activation: `OSC 99 ; i=id ; ST` (button click: `OSC 99 ; i=id ; N ST`),
    only when `a=report` was set;
  - close: `OSC 99 ; i=id : p=close ; [payload] ST`, where payload `untracked`
    means the OS cannot report closure (macOS);
  - alive poll: child sends `OSC 99 ; i=myid : p=alive ; ST`, terminal replies
    `OSC 99 ; i=myid : p=alive ; id1,id2,id3 ST` with the still-alive ids;
  - capability query: `OSC 99 ; i=id : p=? ; ST` → terminal must reply
    `OSC 99 ; i=id : p=? ; k=v : … ST` listing `a` (actions), `c` (close
    events), `o` (occasions), `p` (payload types, ≥ `title`), `s` (sounds),
    `u` (urgencies), `w` (expiry support). Recommended support probe: send the
    query immediately followed by primary DA (`CSI c`); if the DA reply comes
    back with no query reply, unsupported.
- **Security:** terminals *must* sanitize ids before echoing them back
  (injection mitigation); payloads must be escape-code-safe UTF-8 (no C0/C1)
  or base64. An observer that re-emits or republishes ids should apply the
  same sanitization.

## Conceptual distinctions (the load-bearing part)

**Event vs durable current state.** OSC 9/777 are pure events: nothing to
retain beyond the log entry. OSC 99 is *event-driven lifecycle over a
terminal-owned live set*: ids get created, updated, closed, expire. The
proxy's honest model is therefore **an append-only event log** (every parsed
sequence, timestamped — none of these protocols has a heartbeat) plus a
**derived, best-effort outstanding-ids view** (ids seen but not closed).
Ground truth for the outstanding set belongs to the terminal; the proxy can
only become correct about it via close/alive reports, which many terminals
never send. Contrast OSC 7501, where the *record set itself* is the protocol
state and lifetimes are specified; contrast colors, where latest-value-wins
is accurate because the payload is the state.

**Delivery vs observation.** The child emits, the terminal decides whether to
display (focus policy `o`, user filtering, sound). The proxy observes in
transit; it does not deliver. Replaying a notification after respawn — unlike
invisible title re-emit — is a *new interruption event*, not a restore. Any
"missed notifications" summary at reconnect is therefore a separate,
explicitly-requested behavior (out of scope here; flagged for the epic).

**Query replies and capability detection.** OSC 99's `p=?` and `p=alive`
queries carry the `i` key — the spec says it exists *for multiplexers*, so a
middlebox can correlate replies — but correlation is not authorization. The
proxy-owned capability-reply selection recorded in
[shell-tracking0](../tracking/shell-tracking0.gpt6a.md) is a **human
selection for OSC 7501 only**; nothing about it extends to OSC 99. In
particular, answering `p=alive` or `p=?` from the proxy's derived view would
advertise *virtual delivery* semantics the human has not accepted, and a
proxy that answers queries becomes hard to distinguish from a terminal that
really displays notifications. If the proxy ever answers, the reply must be
injected into the stdin relay and ordered before any forwarded primary-DA
reply, or a probing child will conclude "unsupported." Pass-through with
reply sniffing keeps the outer terminal as the single responder. This is a
scope decision inside the OSC 99 child ticket, not settled here; the
record-only observation this design proposes must not be presented as
evidence of actual OS delivery either way.

**IDs and interactions.** Ids address updates, closes, alive polls, and
button/activation reports. Chunking means a "notification created" event may
span several OSC 99 sequences; the log should record parts and the assembler
should publish a completed notification, not per-chunk noise.

**Read/write/roundtrip authority.** OSC 9/777: child→terminal, write-only,
no roundtrip. OSC 99: the child holds write authority (create/update/close)
and read authority (query/alive); the terminal holds reply authority. The
proxy, in the middle, can only *observe* both directions or *intercept*
query/poll replies — it must never synthesize state it does not own (e.g.
answering `p=alive` with the proxy's derived set would misrepresent terminal
state on platforms where notifications were already dismissed).

**Terminal responses (stdin side).** All reverse-channel bytes
(activation/close/alive/query replies) flow outer-terminal → proxy stdin →
child PTY. The current stdin relay is unexamined; observing it is a **new
observation point** this capability needs before any of the reverse channel
becomes visible to the proxy.

**Backend state vs applied terminal.** What the proxy records is what the
child *requested*. Whether the OS showed it, kept it, or silenced it depends
on terminal support (vtdn matrices above), user filters, and focus — none of
it observable from the PTY→stdout stream alone. Record-only observation
**must not be read as an advertisement or proof of actual OS delivery**;
publication should carry the event plus a confidence marker
(emitted-by-child, report-confirmed, derived), never a claim that a toast is
on screen.

## Proposed proxy design

1. **Parser** (`osc.rs`): route params `9`, `777`, `99` to new
   `OscCapture::Notify*` variants. All three are digit params, so the
   existing `osc_param` state machine needs no framing change; OSC 99 bodies
   contain `;` freely, which `osc_data` already tolerates. Sub-parse: OSC 9 →
   check first segment for the ConEmu `digit;` shape (defer to the 9;4 work
   for that branch); OSC 777 → split on `;` into `notify/title/body` with
   missing segments tolerated; OSC 99 → split data at the *first* `;` into
   metadata (then `:`-split pairs) and payload.
   **Cap accounting:** the current `TITLE_CAP = 4096` bounds only captured
   data *after* `ESC ]`, yet a spec-legal OSC 99 chunk may carry up to 4096
   *encoded payload* bytes **plus** the `99 ;` code, metadata section, and
   separator — a maximal chunk does not fit and would be discarded whole as
   overflow today. Notification parsing must size caps against the whole
   body including framing, and pair per-sequence limits with assembled-text
   limits per notification id (chunked assembly can otherwise multiply
   4 KiB chunks without bound), a count bound on tracked ids, and a global
   memory bound. Terminators are BEL or ST as today.
2. **State**: alongside latest-value `ObservedSlots`, a **transient,
   bounded recent-event window** (e.g. ring of N events, each
   `{ts, kind, protocol fields}`) and a derived outstanding-id map for OSC
   99 (created/updated on events, removed on `p=close`, annotated on close
   reports). This is event *observation* for the life of the process —
   **not an owned durable append-only history**: historical retention is
   Kopacetic's responsibility per the division already recorded in
   [shell-tracking0](../tracking/shell-tracking0.gpt6a.md), and eviction
   from the ring must be presented as lossy. The outstanding-id map
   *coalesces* by construction — rapid updates under one id retain the
   latest assembled state, and intermediate events may be dropped under
   pressure — so consumers must treat both views as best-effort recent
   observation, not a complete event history. Snapshots into `SessionExit`;
   deliberately *not* `--reemit-title` state: notifications never re-emit
   implicitly.
3. **Delivery unchanged**: default is observe-and-pass-through. Stripping is
   conceivable (a `notify` filter, mirroring title filters) but is not
   proposed now; notifications are out-of-band effects a user opted into.
4. **Reverse channel (phased)**: phase 1 observes PTY→stdout only. Phase 2
   uses the shared terminal-I/O capability (parent ticket
   **`tipre-terminal-io`**: stdin-side observation, serialized injection,
   and output/query arbitration) so close/activation reports and query
   replies can annotate the log. The `p=?`/`p=alive` responder decision
   (proxy answers vs pass-through) is *not* settled by the 7501 selection
   and must be taken explicitly — including the virtual-delivery concern
   above — before the proxy ever answers on the terminal's behalf.
5. **Publication**: notification events are natural Kopacetic entity updates
   (bounded recent events + outstanding-set correction), but the publication
   boundary, entity shape, and shell entry are parent-owned in
   [shell-tracking0](../tracking/shell-tracking0.gpt6a.md) — this design only
   supplies the event surface, and the proxy's ring is a transient window,
   not the historical record. No duplication intended.
6. **Transport reality**: ssh relays bytes opaquely, but **mosh does not** —
   it runs terminal emulation on both ends and synchronizes screen *state*,
   so output OSC may be consumed, deduplicated, or replayed rather than
   byte-relayed; notification observation placement must not assume
   transparency through mosh. tmux parses OSC as well; its handling of
   9/777/99 is *not retraced here* (9;4 evidence lives in
   [tmux-osc94](../tracking/tmux-osc94.glm53fx.md); envelopes and the
   passthrough ticket qualify further — no new survey). Placement (inside
   vs outside multiplexer/transport) is the same coverage decision the
   tracking epic already records.

## Tickets (pointer, not acceptance)

Epic **`tipre-notifications`**: observe and retain desktop-notification
events raised by the child, across the three wire conventions, with an
accurate event/lifecycle model and a phased reverse channel.

- `tipre-notifications-osc777` — parse and log OSC 777 `notify` events
  (title/body, tolerant arity).
- `tipre-notifications-osc99` — parse kitty OSC 99 (framing, metadata keys,
  chunk assembly, ids), maintain the coalesced outstanding-id view, decide
  and document the `p=?`/`p=alive` responder question against
  `tipre-terminal-io`; includes stdin-side observation via that shared
  capability.
- `tipre-notifications-osc9` — parse and log bare OSC 9 notifications with
  ConEmu subcommand disambiguation (`9;4` stays with the tracking epic).

The event-window surface itself (ring size, snapshot shape, publication
fields) is epic-level scope; reconnect summaries and notification filtering
are explicitly out of scope for all of these. The event log is a transient
recent window — historical retention belongs to Kopacetic, not the proxy.

## References

- Primary: [kitty desktop notifications](https://sw.kovidgoyal.net/kitty/desktop-notifications/);
  kitty source `kitty/notifications.py` (framing confirmed).
- vtdn pages (AI-generated placeholders, cross-checked where load-bearing):
  [OSC 9](https://vtdn.dev/docs/osc/osc9),
  [OSC 777](https://vtdn.dev/docs/osc/osc777),
  [kitty OSC 99](https://vtdn.dev/docs/osc/kitty-osc99).
- In-repo: [OSC 9;4 / 7501 brief](../tracking/osc-protocols.glm53m.md)
  (9-vs-9;4 collision, 7501 lifetimes),
  [shell tracking design](../tracking/shell-tracking0.gpt6a.md)
  (entity/publication, responder selection),
  [tmux OSC 9;4 trace](../tracking/tmux-osc94.glm53fx.md).
