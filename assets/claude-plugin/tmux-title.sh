#!/bin/sh
# Keep a forestui tmux window and the Claude session running in it under one
# name, in both directions: renaming the tab renames the session, and /rename
# renames the tab.
#
# Installed and removed by forestui; edits here are reported as drift rather
# than silently overwritten.
#
# The two names are the same string, verbatim. Nothing is added, stripped or
# interpreted — a window called "yolo:forestuiNAMESYNC" holds a session called
# "yolo:forestuiNAMESYNC". Prefixes are forestui's opening move when it creates
# a window, not a format anything downstream parses.
#
# Two-way sync has to know which side moved, which one value cannot tell it:
# equal names are ambiguous between "nobody changed" and "both changed to the
# same thing", and unequal names do not say who is stale. So the last agreed
# name is kept in @claude_synced_name and both sides are compared against it.
# When both moved, the tab wins — it is the one the user physically renamed,
# and a session can always be renamed again.
#
# The event name arrives as $1 rather than being parsed out of stdin. Only
# SessionStart and UserPromptSubmit are allowed to set a session title, so Stop
# can push a name outward to the tab but never inward.
set -u

event="${1:-}"
input=$(cat)
[ -n "$event" ] || exit 0

# There is deliberately no off switch, per window or otherwise. Installed means
# the two names agree; not wanting that is an uninstall, decided once.
#
# Guards, narrowing. Not inside tmux at all — a bare `claude` in a plain
# terminal — stops here, before tmux is ever invoked.
[ -n "${TMUX_PANE:-}" ] || exit 0

# Reading the session's own name needs a JSON parser. Without one the sync
# would be half-blind, so do nothing rather than guess: forestui's installer
# checks for jq and says so when it is missing.
command -v jq >/dev/null 2>&1 || exit 0

# The stamp answers one question only: did forestui open this window? A tmux
# window the user made themselves carries none and is left alone. It is not a
# claim marker — every window forestui opens syncs, from its first turn.
birth=$(tmux show-options -wqv -t "$TMUX_PANE" @claude_birth_name 2>/dev/null) || exit 0
[ -n "$birth" ] || exit 0

# Control characters are stripped from both names before anything is done with
# them: a newline inside a JSON string is invalid JSON, so a window somebody
# renamed with one would turn every prompt into a hook error.
window=$(tmux display-message -p -t "$TMUX_PANE" '#{window_name}' 2>/dev/null | tr -d '\000-\037') || exit 0
[ -n "$window" ] || exit 0

# Empty when the session has no name. Generated titles are not reported here,
# only names set deliberately, which is why an auto-titled session never
# renames its tab.
title=$(printf '%s' "$input" | jq -r '.session_title // empty' 2>/dev/null | tr -d '\000-\037')
synced=$(tmux show-options -wqv -t "$TMUX_PANE" @claude_synced_name 2>/dev/null)

mode=none
if [ -z "$synced" ]; then
  # First reconciliation for this window. A session that already has a name
  # keeps it and names the tab — that is a resume. Otherwise the tab names the
  # session, which is what gives every forestui window a session name from the
  # start rather than only once somebody renames something.
  if [ -n "$title" ]; then
    mode=adopt
  else
    mode=push
  fi
elif [ "$window" != "$synced" ]; then
  mode=push
elif [ -z "$title" ]; then
  # The window agrees with the last sync but the session has no name. Either a
  # new session started in a window an earlier one used, or a push was recorded
  # and never took. Pushing again covers both and costs nothing when the name
  # is already right, because Claude ignores a title equal to the current one.
  mode=push
elif [ "$title" != "$synced" ]; then
  mode=adopt
fi

case "$mode" in
  adopt)
    tmux rename-window -t "$TMUX_PANE" "$title" 2>/dev/null || exit 0
    tmux set-option -w -t "$TMUX_PANE" @claude_synced_name "$title" 2>/dev/null
    ;;
  push)
    # Only on a prompt, never at SessionStart. A title set before the UI is
    # live is stored but never drawn: the name badge on the input box, which is
    # the one place Claude shows a session's name, renders from state the
    # session picks up while running. Setting it at SessionStart therefore
    # produced a correctly named session that looked unnamed. Waiting for the
    # first prompt names it on turn one and draws the badge, exactly as
    # /rename does. Stop cannot set a title at all.
    case "$event" in
      UserPromptSubmit) ;;
      *) exit 0 ;;
    esac
    tmux set-option -w -t "$TMUX_PANE" @claude_synced_name "$window" 2>/dev/null
    escaped=$(printf '%s' "$window" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')
    printf '{"hookSpecificOutput":{"hookEventName":"%s","sessionTitle":"%s"}}\n' "$event" "$escaped"
    ;;
esac
exit 0
