---
type: Research
title: "OSC 9;4 progress bar in tmux: parser, state, propagation, and term-preserver implications"
description: "Source-grounded study of tmux's OSC 9;4 handling (input parsing, screen state, format variables, Spb feature gating, client propagation) and byte-path analysis of term-preserver as proxy outside tmux (sees re-emitted selected-pane projection) vs proxy inside tmux (observes raw child sequences before tmux); notes absent OSC 7501 handler in the inspected revision and the DCS passthrough exception. Rev 2: corrected proxy position naming, qualified version/absence claims, precise indeterminate retention semantics."
resource: /home/rektide/src/term-preserver/design/tracking/tmux-osc94.glm53fx.md
status: draft
author: model:zai-coding-plan/glm-5.3-flash#max (subagent, petname nimble-panda)
date: 2026-10-14
sources:
  - resource: "opencode://ses_ec843ae4bffeC3iIfhQaSKCCM9"
    title: "FX investigation of OSC 9;4 in archived tmux next-3.9 (parser/state/propagation mapping, byte-path proxy-outside/inside-tmux analysis); rev 2 corrections from review: proxy position diagrams, qualified version/absence claims, DCS passthrough exception, indeterminate retention semantics"
  - resource: "/home/rektide/archive/tmux/tmux/input.c"
    title: "tmux input parser: input_osc_9, input_exit_osc dispatch, input_set_progress_bar"
  - resource: "/home/rektide/archive/tmux/tmux/screen.c"
    title: "tmux screen state: screen_set_progress_bar, screen_reinit reset"
  - resource: "/home/rektide/archive/tmux/tmux/tmux.h"
    title: "tmux progress_bar_state enum, progress_bar struct, TTYC_SPB"
  - resource: "/home/rektide/archive/tmux/tmux/tty-features.c"
    title: "tmux progressbar tty feature and terminal-type feature table"
  - resource: "/home/rektide/archive/tmux/tmux/tty.c"
    title: "tmux tty_set_progress_bar emission"
  - resource: "/home/rektide/archive/tmux/tmux/server-client.c"
    title: "tmux server_client_set_progress_bar client propagation"
  - resource: "/home/rektide/archive/tmux/tmux/format.c"
    title: "tmux pane_pb_state / pane_pb_progress format callbacks"
  - resource: "/home/rektide/archive/tmux/tmux/tmux.1"
    title: "tmux manual: progressbar feature, pane_pb_* formats, Spb capability"
  - resource: "/home/rektide/archive/tmux/tmux/regress/input-osc.sh"
    title: "tmux regress test covering OSC 9;4"
  - resource: "/home/rektide/src/term-preserver/src/osc.rs"
    title: "term-preserver OSC parser (no OSC 9 handling)"
  - resource: "/home/rektide/src/term-preserver/src/tmux.rs"
    title: "term-preserver tmux poll sampler"
  - resource: "/home/rektide/src/term-preserver/src/session.rs"
    title: "term-preserver PTY→stdout relay pass-through behavior"
---

# OSC 9;4 in tmux — research notes

Reference tree: `/home/rektide/archive/tmux/tmux` (read-only upstream archive),
HEAD `f0a2ed30` = `3.7b-1138-gf0a2ed30`, `AC_INIT([tmux], next-3.9)` in
`configure.ac`, newest tag `3.8-rc`. All line numbers below are from this
checkout.

## 1. Parser semantics (`input_osc_9`)

- Dispatch: `input_exit_osc()` (input.c:2704) splits the OSC number from the
  remainder (`p`), then `case 9:` calls `input_osc_9(ictx, p)`
  (input.c:2749–2751). Both BEL and ST terminators are accepted upstream
  (`ictx->input_end`, input.c:2708).
- Handler: `input_osc_9()` (input.c:3033–3071). Grammar it accepts:
  `OSC 9 ; 4 ; <state>[;<progress>]` where:
  - First payload char must be `4` — a bare `OSC 9` (ConEmu-style notify) is
    **silently ignored** (`if (*pb++ != '4') return;`, input.c:3040).
  - `OSC 9;4` or `OSC 9;4;` alone: ignored, no state change
    (input.c:3040–3041).
  - `<state>` must be a single digit `0`–`4`; anything else (including `?` —
    there is **no query form**) goes to `bad:` and the whole sequence is
    dropped with only a `log_debug` (input.c:3045–3047, 3070).
  - `<progress>`, when present, must be pure decimal digits and ≤ 100
    (leading zeros fine); `9;4;5;200` or `9;4;1;2x` are rejected wholesale
    (input.c:3058–3070).
  - `OSC 9;4;<state>` with no progress field passes `-1`, which updates the
    **state only and preserves the previous progress**
    (input.c:3052–3055 → screen.c:316–322: "progress will not be updated if
    p is negative").
- No option gating: unlike titles (`allow-set-title`, input.c:2730–2731),
  OSC 9;4 is always parsed regardless of any option.

## 2. State, defaults, reset

- Types (tmux.h):
  - `enum progress_bar_state { HIDDEN=0, NORMAL=1, ERROR=2, INDETERMINATE=3,
    PAUSED=4 }` (tmux.h:1055–1061) — the numeric values match the wire codes.
  - `struct progress_bar { enum progress_bar_state state; int progress; }`
    (tmux.h:1062–1065).
  - Stored on the pane's base screen: `screen.progress_bar`
    (tmux.h:1104), plus a per-client cached copy `client.progress_bar`
    (tmux.h:2231).
- `screen_set_progress_bar()` (screen.c:316–322): sets state always; the
  stored progress is overwritten only when `p >= 0` **and** state ≠
  `PROGRESS_BAR_INDETERMINATE`. Two distinct retention paths, not to be
  conflated:
  - *State-only update* (`OSC 9;4;<state>`, p = −1, input.c:3052–3055):
    progress retained, for every state.
  - *INDETERMINATE with a supplied percentage* (`OSC 9;4;3;<n>`): the
    percentage is parsed and accepted on the wire, but the stored value is
    left unchanged — an indeterminate bar **retains** whatever percentage was
    last stored (set `9;4;1;25` then `9;4;3;80`: `pane_pb_progress` stays
    25). It is not zeroed; "indeterminate" governs *updates*, not the
    existence of a stored value.
- Default: `screen_reinit()` sets `PROGRESS_BAR_HIDDEN, 0`
  (screen.c:127; `screen_init` calls `screen_reinit` at screen.c:23).
- Reset behavior:
  - **Not** reset by alternate-screen enter/exit
    (screen.c:661 `screen_alternate_on`, screen.c:687
    `screen_alternate_off` touch only the grid/cursor) — an app that starts a
    progress bar, suspends via `smcup`, and exits leaves the bar state behind.
  - There is **no OSC 110-family reset and no query** for progress; the
    canonical "hide" is `OSC 9;4;0` (optionally `;0`).
  - Reset to HIDDEN/0 happens on screen reinit (pane respawn, RIS-class
    resets).

## 3. Exposure: format variables

- `pane_pb_state` → `hidden|normal|error|indeterminate|paused`;
  `pane_pb_progress` → `%d` (format.c:2621–2650, table entries
  format.c:3947–3951). Documented at tmux.1:7563–7564.
- These make pane-level progress state observable from *outside* tmux
  without byte sniffing (see §7, proxy-OUTSIDE position).

## 4. Propagation to the outside terminal

Two-stage design — parse is unconditional; **emission is feature-gated**:

1. On parse, `input_set_progress_bar()` (input.c:3021–3029) stores the state
   and forces `server_redraw_window_borders` + `server_status_window`, so a
   client redraw happens promptly.
2. During each client's full redraw (`c->flags & CLIENT_ALLREDRAWFLAGS`,
   server-client.c:2474–2482), `server_client_set_progress_bar()`
   (server-client.c:2544–2558) compares the **active pane of the client's
   current window** (`s->curw->window->active->base.progress_bar`) against
   the client cache; on change it emits `tty_set_progress_bar()`.
3. `tty_set_progress_bar()` (tty.c:2963–2967) writes the sequence **only if**
   the term has `TTYC_SPB` (tmux.h:670, name table tty-term.c:288):
   `tty_putcode_ii(TTYC_SPB, state, progress)`.

The `Spb` capability is supplied by the `progressbar` **tty feature**:
`Spb=\E]9;4;%p1%d;%p2%d\E\\` (tty-features.c:379–387, feature list entry at
tty-features.c:414). Features come from the terminal-type table
(tty-features.c:595–700) or the `terminal-features` option (applied at
tty-term.c:619–629, re-applied after overrides at tty-term.c:687). In the
inspected table: entries with `progressbar` are **tmux, iTerm2, ghostty,
Rio**; entries without it include foot, WezTerm, kitty, XTerm, rxvt-unicode
(each list verified in this checkout, including XTerm's full entry). Manual:
tmux.1:5215–5216 (`progressbar`), tmux.1:9024 (`Spb`). What the table asserts
is that those terminal *types* advertise `Spb` to tmux; how each terminal
itself consumes or renders the sequence is upstream of tmux and not evidenced
in this tree.

Consequences of the model (all as coded in the inspected revision):

- The bar shown on the real terminal is tmux's **projection of one pane**
  (active pane of the focused window). A background-window pane emitting
  9;4 does not move the outer bar (its state is stored, format-visible, but
  not emitted until that pane is active and a full redraw fires).
- Emission is deferred/deduplicated through the client cache and only rides
  full redraws — not a 1:1 byte echo of the app's sequences.

## 5. Version history and tests

- `bdd78ce3` 2026-04-03 — "Handle OSC 9;4 progress bar sequence and store in
  format variables, from Eric Dorland in GitHub issue 4954." (parser +
  formats)
- `bc15723f` 2026-04-22 — "Add feature for progress bar and pass to outside
  terminal", GitHub issue 4972. (progressbar feature / Spb emission)
- Both commits are contained in tags **3.7, 3.7a/b/c, 3.8-rc**
  (`git tag --contains`): OSC 9;4 support (including outside propagation) is
  present in the **3.7** tag. Revisions before `bdd78ce3` have no `case 9`
  OSC handler, so the sequence falls through to the unknown-OSC default
  (log-and-drop, input.c:2778–2781) — stated as an inference from dispatch
  structure, not verified against each older tree.
- Test: `regress/input-osc.sh:16` —
  `start_pane progress 20 3 '\033]9;4;1;25\007\033]9;4;0\007\033]9;4;5;200\007X\n'`
  then `check_capture progress 'X'`: valid set, valid hide, and an invalid
  `5;200` must all be swallowed without corrupting pane output (no sequence
  leakage into the grid).

## 6. OSC 7501 (secondary note)

No occurrence of the `7501` OSC number in the inspected revision
(`f0a2ed30`; `rg` over `*.c`, `*.h`, `tmux.1`). An app emitting it hits
`default: log_debug(...)` (input.c:2778–2781) and the sequence is discarded —
in this tree, inside tmux it reaches neither the pane model nor the outer
terminal. This establishes absence of a handler **in the pinned revision
only**; it is not evidence about other tmux versions, and no grammar or
terminal attribution for 7501 is claimed here — that belongs to the dedicated
7501 research.

Explicit exception in the same revision: DCS passthrough. A pane program can
send `DCS tmux;<payload> ST`; when the pane option `allow-passthrough` is
`on` (visible pane) or `all` (any pane), `input_dcs_dispatch` forwards the
payload verbatim to the outer terminal via `screen_write_rawstring`
(input.c:2678–2694; option choices/default `off` at options-table.c:1283–1291
with `default_num = 0`; tmux.1:6378–6386). Payload bytes bypass tmux's
re-modeling entirely, so any sequence — 9;4 included — can cross tmux
untranslated when the option is set.

## 7. Implications for term-preserver (proxy outside vs inside tmux)

term-preserver today (both positions):

- Parser: `src/osc.rs` `finish_osc()` matches params 0/1/2, 4/5, 10/11/12,
  104/105, 110/111/112 (osc.rs:297–395); param `"9"` falls to `_ => None`
  (osc.rs:391) — **OSC 9;4 is not captured** in either position.
- Relay: `copy_pty_to_stdout_with_titles()` (session.rs:232–260) passes all
  bytes through except stripped text titles; whatever enters the proxy's pty
  leaves its stdout verbatim.

Which byte population the proxy sees depends on where it sits relative to
tmux. The two configurations:

### Proxy OUTSIDE tmux — `terminal → term-preserver → tmux client → pane app`

term-preserver's pty wraps the tmux client; this is the launch-path mode
(`launch.rs:201–218` builds `tmux new -As …` / attach commands as the
proxy's child).

```text
pane app ──raw \E]9;4;…──▶ tmux server parser
                            (input_osc_9: consumed, stored per-pane, §1–2)
                               │  re-modeling: projection of the focused pane
                               ▼
                tmux client render path
                (server_client_set_progress_bar: active pane of the client's
                 current window, full-redraw gated, Spb-gated — §4)
                               │  re-emitted \E]9;4;state;progress\E\\
                               ▼
                term-preserver pty (proxy) ──▶ terminal
```

- What the proxy observes is **tmux's re-emitted selected-pane projection**,
  subject to all §4 conditions (active pane of the current window only, full
  redraws only, outer terminal must advertise `Spb`). The re-emitted form
  *does* appear in the proxy's byte stream; what never appears is the raw
  per-pane stream of the app.
- Byte capture here would track the projection, not per-pane truth. Per-pane
  state beyond the projection is only available through tmux introspection:
  `#{pane_pb_state}` / `#{pane_pb_progress}` (§3) via the existing poll
  sampler (`tmux.rs:171–183`, 20s `SAMPLE_INTERVAL` at tmux.rs:187–191) or
  the anticipated `control` source (tmux.rs:47–51).

### Proxy INSIDE tmux — `terminal → tmux pane → term-preserver → zsh/app`

term-preserver runs as the pane command, its pty wrapping the shell/app.

```text
zsh/app ──raw \E]9;4;…──▶ term-preserver pty (proxy)
                            ◀─ observed here: raw child sequences,
                               BEFORE tmux consumes them
                               │  relay forwards verbatim (no 9;4 filtering)
                               ▼
                tmux pane pty ──▶ tmux server parser (consumes, stores)
                               │  projection per §4
                               ▼
                tmux client ──▶ terminal
```

- The proxy observes the **raw child sequences before tmux sees them** —
  ground truth for that pane. Byte capture here is structurally possible and
  would mirror tmux's own parser grammar (§1–2: state 0–4, optional 0–100
  decimal, preserve-progress on state-only updates, HIDDEN default).
- Everything the proxy forwards is then consumed by tmux; the user's
  terminal shows tmux's projection, not the child's stream. If the proxy
  itself ever **emits** 9;4, the same mediation applies: revisions with the
  handlers (tag 3.7 onward) re-project per §4; earlier revisions drop it via
  the unknown-OSC default (§5).
- Explicit escape from that mediation: DCS `tmux;…ST` passthrough (§6) —
  child bytes that cross tmux untranslated when `allow-passthrough` is set
  (default `off`).

### No tmux

`terminal → term-preserver → app`: the proxy sees raw sequences and the
terminal sees raw bytes; capture and outer visibility coincide.

Version gate (either tmux position): `pane_pb_*` formats and the
`progressbar` feature exist only in trees containing `bdd78ce3`/`bc15723f`
(tag **3.7** onward); poll/control code should treat a missing
`pane_pb_state` as "tmux predates support", not an error.

## 8. Summary table

| Aspect | tmux behavior |
| --- | --- |
| Parse | `OSC 9;4;<state>[;<prog>]`, state 0–4, prog 0–100 decimal; always parsed, no option gate, no query |
| Bare OSC 9 / bad payload | silently dropped (`log_debug`) |
| State-only update | `9;4;<state>` keeps prior progress (p = −1 suppresses progress write) |
| Indeterminate | stored percentage retained (updates ignored); state-only path also retains — same value, different trigger |
| Default / reset | HIDDEN/0 at screen init/reinit; survives alt-screen; no OSC reset verb (use `9;4;0`) |
| Formats | `pane_pb_state`, `pane_pb_progress` |
| Outside emission | only on full client redraws, active pane of current window, only when term has `Spb` (inspected table: tmux, iTerm2, ghostty, Rio) |
| Since | present in tag 3.7 (commits bdd78ce3 2026-04-03, bc15723f 2026-04-22); earlier revisions inferred to drop via unknown-OSC default |
| Passthrough | DCS `tmux;…ST` bypasses re-modeling when `allow-passthrough` on/all (default off) |
| OSC 7501 | no handler in the inspected revision `f0a2ed30`; absence not established for other revisions |
