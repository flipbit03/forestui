#!/bin/sh
# Record where a Claude session is running, so forestui can see sessions it
# did not start — a `claude -r <id>` typed into a hand-made tmux window, or a
# terminal with no tmux at all. The window-option stamp only covers windows
# forestui opened; this hook runs inside *every* session on the machine, which
# is the one vantage point that sees them all.
#
# Installed and removed by forestui; edits here are reported as drift rather
# than silently overwritten.
#
# One file per session under ~/.config/forestui/live/, written on SessionStart
# and refreshed on UserPromptSubmit (which also covers a plugin installed
# mid-session), removed on SessionEnd. The file records the *claude* process
# id — found by walking up from this hook, which runs under a shell claude
# spawned and threw away — so forestui can tell a live session from a
# heartbeat a crash left behind: a dead pid, or a reused one that is not
# claude, reads as stale and is swept.
set -u

event="${1:-}"
input=$(cat)
[ -n "$event" ] || exit 0

# The session id comes out of the hook's JSON input, so this needs the same
# JSON parser the title sync needs. Without it, do nothing rather than guess.
command -v jq >/dev/null 2>&1 || exit 0

# The id names a file, so nothing but the characters a session id can contain
# survives — a hostile hook input must not traverse out of the live directory.
sid=$(printf '%s' "$input" | jq -r '.session_id // empty' 2>/dev/null | tr -cd 'a-zA-Z0-9-')
[ -n "$sid" ] || exit 0

dir="$HOME/.config/forestui/live"

if [ "$event" = "SessionEnd" ]; then
  rm -f "$dir/$sid.json" 2>/dev/null
  exit 0
fi

# Walk up to the claude process this hook belongs to. The hook's own parent is
# a shell claude spawned for the command, which dies the moment the hook ends —
# recording *its* pid would make every heartbeat instantly stale.
pid=$$
found=""
i=0
while [ "$i" -lt 6 ]; do
  pid=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ')
  case "$pid" in '' | 0 | 1) break ;; esac
  comm=$(ps -o comm= -p "$pid" 2>/dev/null | tr -d ' ')
  case "$comm" in
    claude* | node*)
      found="$pid"
      break
      ;;
  esac
  i=$((i + 1))
done
# No claude in the ancestry: write nothing. A heartbeat that cannot be
# validated later is a lie waiting to be believed.
[ -n "$found" ] || exit 0

mkdir -p "$dir" 2>/dev/null || exit 0

cwd=$(printf '%s' "$input" | jq -r '.cwd // empty' 2>/dev/null)
pane="${TMUX_PANE:-}"

# Written whole then renamed, so forestui never reads half a file.
tmp="$dir/.$sid.$$.tmp"
printf '{"session_id":"%s","pid":%s,"tmux_pane":"%s","cwd":%s}\n' \
  "$sid" "$found" "$pane" "$(printf '%s' "$cwd" | jq -R .)" \
  > "$tmp" 2>/dev/null || { rm -f "$tmp" 2>/dev/null; exit 0; }
mv -f "$tmp" "$dir/$sid.json" 2>/dev/null || rm -f "$tmp" 2>/dev/null

# Heal the window stamp while we are here: whatever an earlier launch stamped
# on this window, the session running in this pane is *this* one — a manual
# `claude -r <other-id>` in a forestui window would otherwise wear the old
# session's badge until the window closed.
if [ -n "$pane" ]; then
  tmux set-option -w -t "$pane" @claude_session_id "$sid" 2>/dev/null || true
fi
exit 0
