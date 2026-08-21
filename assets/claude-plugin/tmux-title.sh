#!/bin/sh
# Name the Claude session after the tmux window forestui opened it in.
#
# Installed and removed by forestui; edits here are reported as drift rather
# than silently overwritten.
#
# The window name is the source of truth, but only once somebody has claimed
# it. forestui stamps @claude_birth_name when it creates the window, and this
# hook stays silent while the window still carries that name — so an untouched
# tab leaves the session alone and Claude goes on auto-titling it. Renaming the
# tab is what hands the name over.
#
# The event name arrives as $1 rather than being parsed out of stdin, so the
# only thing this needs jq for is an optimisation.
set -u

event="${1:-}"
input=$(cat)
[ -n "$event" ] || exit 0

# There is deliberately no per-window or global off switch. Installed means
# tabs and sessions agree; not wanting that is an uninstall, which is one
# decision made in one place instead of a lever to remember per tab.
#
# Three guards, narrowing. Not inside tmux at all — a bare `claude` in a plain
# terminal — stops here, before tmux is ever invoked.
[ -n "${TMUX_PANE:-}" ] || exit 0

# No stamp means forestui did not open this window, so its name is not ours to
# read: a tmux window the user made themselves is left alone.
birth=$(tmux show-options -wqv -t "$TMUX_PANE" @claude_birth_name 2>/dev/null) || exit 0
[ -n "$birth" ] || exit 0

window=$(tmux display-message -p -t "$TMUX_PANE" '#{window_name}' 2>/dev/null) || exit 0
[ -n "$window" ] || exit 0
[ "$window" = "$birth" ] && exit 0

# Drop whatever prefix forestui put on the window (claude:, yolo:, or a custom
# button's), taken from the stamp so any prefix works. A name may itself
# contain a colon, so only the first one is a separator.
prefix=${birth%%:*}
case "$window" in
  "$prefix":*) title=${window#"$prefix":} ;;
  *) title=$window ;;
esac
[ -n "$title" ] || exit 0

# Claude ignores a title equal to the current one; skipping it here just avoids
# a pointless transcript entry on every prompt.
if command -v jq >/dev/null 2>&1; then
  current=$(printf '%s' "$input" | jq -r '.session_title // empty' 2>/dev/null)
  [ "$title" = "$current" ] && exit 0
fi

escaped=$(printf '%s' "$title" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')
printf '{"hookSpecificOutput":{"hookEventName":"%s","sessionTitle":"%s"}}\n' "$event" "$escaped"
