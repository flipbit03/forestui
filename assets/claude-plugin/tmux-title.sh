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

# No stamp means forestui did not open this window, so its name is not ours to
# read or write: a tmux window the user made themselves is left alone.
birth=$(tmux show-options -wqv -t "$TMUX_PANE" @claude_birth_name 2>/dev/null) || exit 0
[ -n "$birth" ] || exit 0

window=$(tmux display-message -p -t "$TMUX_PANE" '#{window_name}' 2>/dev/null) || exit 0
[ -n "$window" ] || exit 0

# Empty when the session has no name. Generated titles are not reported here,
# only names set deliberately, which is why an auto-titled session never
# renames its tab.
title=$(printf '%s' "$input" | jq -r '.session_title // empty' 2>/dev/null)
synced=$(tmux show-options -wqv -t "$TMUX_PANE" @claude_synced_name 2>/dev/null)

# An untouched tab and an unnamed session: nothing to agree on yet. Leaving
# both alone is what lets Claude go on titling the session itself.
if [ "$window" = "$birth" ] && [ -z "$title" ]; then
  exit 0
fi

mode=none
if [ -z "$synced" ]; then
  # First reconciliation for this window. A tab still carrying the name
  # forestui gave it has not been claimed, so it adopts the session's name;
  # a tab already renamed claims the session.
  if [ "$window" = "$birth" ]; then
    [ -n "$title" ] && mode=adopt
  else
    mode=push
  fi
elif [ "$window" != "$synced" ]; then
  mode=push
elif [ -n "$title" ] && [ "$title" != "$synced" ]; then
  mode=adopt
fi

case "$mode" in
  adopt)
    tmux rename-window -t "$TMUX_PANE" "$title" 2>/dev/null || exit 0
    tmux set-option -w -t "$TMUX_PANE" @claude_synced_name "$title" 2>/dev/null
    ;;
  push)
    # Stop cannot set a title; the next prompt picks this up.
    case "$event" in
      SessionStart | UserPromptSubmit) ;;
      *) exit 0 ;;
    esac
    tmux set-option -w -t "$TMUX_PANE" @claude_synced_name "$window" 2>/dev/null
    escaped=$(printf '%s' "$window" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')
    printf '{"hookSpecificOutput":{"hookEventName":"%s","sessionTitle":"%s"}}\n' "$event" "$escaped"
    ;;
esac
exit 0
