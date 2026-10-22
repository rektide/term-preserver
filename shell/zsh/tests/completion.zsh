#!/usr/bin/env zsh
# Real completion widgets in an isolated PTY; no launch command is submitted.
# Adapted from the tta completion harness: the binary is symlinked as tper,
# the project root comes from TTA_ROOT, and session checks pin the classic
# tmux CLI backend so the inventory does not depend on host tta availability.
emulate -LR zsh
setopt err_exit no_unset pipe_fail
zmodload zsh/zpty
zmodload zsh/zselect

binary=$1
export TPER_COMPLETION_FUNCTIONS=$2
fixture=$3
export TPER_LIVE_SOCKET=$4
mkdir -p "$fixture/bin" "$fixture/root/pipewire-native" "$fixture/root/space project" \
  "$fixture/root/meta[x]*" "$fixture/root/quote'project" "$fixture/root/--dash" \
  "$fixture/override root/other-project"
print -r -- 'not a socket' > "$fixture/bad-socket"
ln -s "$binary" "$fixture/bin/tper"
export PATH="$fixture/bin:$PATH" HOME="$fixture"
for key in ${(k)parameters[(I)TERM_PRESERVER_*]}; do
  unset "$key"
done
for key in ${(k)parameters[(I)TTA_*]}; do
  [[ $key == (TTA_ROOT) ]] || unset "$key"
done
unset PROJECTS_DIR TMUX TMUX_PANE
export TTA_ROOT="$fixture/root"
export TPER_CAPTURE="$fixture/capture"
# Pin the classic CLI backend and a dead socket for unpinned queries: the
# inventory then never depends on host tta availability or the operator's live
# default server. Explicit --tmux-socket flags override the dead endpoint.
export TERM_PRESERVER_LAUNCH__SESSION_BACKEND=classic-tmux-cli
export TERM_PRESERVER_LAUNCH__TMUX_SOCKET="$fixture/dead-socket"
socket_probe="$fixture/probe-socket"

zpty -b completion env TERM=xterm zsh -f
trap 'zpty -d completion' EXIT
zpty -w completion 'PS1="READY> "; fpath=( "$TPER_COMPLETION_FUNCTIONS" $fpath ); autoload -Uz compinit; compinit -D; bindkey "^I" complete-word; _capture() { print -r -- "$BUFFER" > "$TPER_CAPTURE"; BUFFER=""; zle .accept-line }; zle -N _capture; bindkey "^X" _capture; print INITIALIZED'

transcript=''
wait_for() {
  local needle=$1 chunk i
  for i in {1..1000}; do
    if zpty -r completion chunk; then
      transcript+=$chunk
      [[ $transcript == *$needle* ]] && return 0
    fi
    zselect -t 1 || true
  done
  print -u2 -r -- "Timed out waiting for $needle: $transcript"
  return 1
}
wait_for INITIALIZED

check() {
  local input=$1 expected=$2 i actual
  rm -f "$TPER_CAPTURE"
  zpty -w -n completion "$input"$'\t\x18'
  for i in {1..1000}; do
    if [[ -f $TPER_CAPTURE ]]; then
      actual=$(<"$TPER_CAPTURE")
      if [[ $actual != "$expected" ]]; then
        print -u2 -r -- "Input ${(qqq)input}: expected ${(qqq)expected}, got ${(qqq)actual}"
        return 1
      fi
      local chunk
      while zpty -r completion chunk; do :; done
      return 0
    fi
    zselect -t 1 || true
  done
  print -u2 -r -- "Timed out completing ${(qqq)input}"
  return 1
}

# Session-target values complete against sessions and projects, with wildcard
# and substring matching like tta. The backend is pinned so the inventory is
# independent of host tta presence.
check 'tper --tmux pipe' 'tper --tmux pipewire-native '
check 'tper --tmux wire' 'tper --tmux pipewire-native '
check 'tper --tmux *wire*nat*' 'tper --tmux pipewire-native '
check 'tper --tta p?pewire-nat*' 'tper --tta pipewire-native '
check 'tper --tmux-create space' 'tper --tmux-create space\ project '
check 'tper --tmux-attach meta' 'tper --tmux-attach meta\[x\]\* '
check 'tper --tmux quote' "tper --tmux quote\\'project "
check 'tper --session-roo' 'tper --session-root='
check 'tper --tta-progra' 'tper --tta-program='
check "tper --session-root '$fixture/override root' --tmux oth" "tper --session-root '$fixture/override root' --tmux other-project "
check "tper --session-root='$fixture/override root' --tmux oth" "tper --session-root='$fixture/override root' --tmux other-project "
check 'tper --session-backend aut' 'tper --session-backend auto '
check "tper --tmux-socket '$socket_probe' --session-backend classic-tmux-cli --tmux wire" "tper --tmux-socket '$socket_probe' --session-backend classic-tmux-cli --tmux pipewire-native "
check "tper --session-backend classic-tmux-cli --tmux-socket '$TPER_LIVE_SOCKET' --tmux live" "tper --session-backend classic-tmux-cli --tmux-socket '$TPER_LIVE_SOCKET' --tmux live-work "
check "tper --session-backend classic-tmux-cli --tmux-socket '$TPER_LIVE_SOCKET' --tmux *work" "tper --session-backend classic-tmux-cli --tmux-socket '$TPER_LIVE_SOCKET' --tmux live-work "
check "tper --session-backend classic-tmux-cli --tmux-socket '$fixture/bad-socket' --tmux wire" "tper --session-backend classic-tmux-cli --tmux-socket '$fixture/bad-socket' --tmux pipewire-native "
# Inventory queries never create a socket: neither the probe endpoint nor the
# pinned dead endpoint may exist afterwards.
[[ ! -e $socket_probe ]]
[[ ! -e $TERM_PRESERVER_LAUNCH__TMUX_SOCKET ]]
