# term-preserver agent guidance

Testing here often drives a real `tmux`. On 2026-10-21 a scratch "isolated"
server wasn't: `tmux kill-server` run under `TMUX_TMPDIR` (observed silently
ignored by the local tmux 3.7c client) hit the default socket and destroyed
the user's live sessions. The rules below follow from that incident.

- **Isolate with an explicit absolute socket**: `tmux -S /tmp/opencode/<topic>/sock …`.
  Not bare `tmux` (the user's default server), not `-L <name>` (socket in the
  shared default dir), not `TMUX_TMPDIR` (unreliable here — assume nothing
  about it).
- **Prove isolation read-only before any mutation.** `tmux -S <path> ls` fails
  with `no server running on <path>` without starting anything; the path in
  that error must be exactly yours, or stop.
- **Prefer the narrowest destructive tool.** Scratch cleanup rarely needs
  `kill-server`: `kill-session -t <name-you-created>` or letting the server
  exit via `exit-empty` usually suffices. A mis-aimed narrow command errors
  harmlessly; a mis-aimed broad one destroys work.
- **Run `kill-server` only against a socket you created and verified this
  session, and never hide its stderr.** The wrong-server evidence is exactly
  the output you need to see.
- **To exercise scripts that call bare `tmux` end-to-end**, put a PATH shim
  (`exec /usr/bin/tmux -S /tmp/opencode/<topic>/sock "$@"`) ahead of the real
  binary rather than altering the environment of the user's server.
