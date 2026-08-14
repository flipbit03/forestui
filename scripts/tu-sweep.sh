#!/usr/bin/env bash
#
# tu-sweep.sh — drive forestui through the UC-53..UC-70 hotkey sweep and capture
# one PNG plus one text frame per case.
#
# The text frames are the comparable artifact: they diff cleanly, so the same
# sweep run against two builds shows exactly which screens differ. The PNGs are
# for human eyeballing (colour, focus highlight) and are not committed.
#
#   usage: scripts/tu-sweep.sh <label> <forestui-command...>
#
#   examples:
#     scripts/tu-sweep.sh rust    ./target/release/forestui
#     scripts/tu-sweep.sh python  /path/to/.venv/bin/forestui
#
# Output:
#   doc/rust-rewrite/baseline/<label>/UC-NN-*.txt   committed, diffable
#   doc/rust-rewrite/screenshots/<label>/UC-NN-*.png gitignored
#
# The sweep builds its own throwaway forest, HOME and tmux server, and stubs
# vim/mc/claude so the window-creating hotkeys produce windows that stay alive
# long enough to be named. It never touches the caller's ~/forest or tmux.

set -uo pipefail

LABEL="${1:?usage: tu-sweep.sh <label> <forestui-command...>}"
shift
FUI_CMD=("$@")
[ "${#FUI_CMD[@]}" -gt 0 ] || { echo "no forestui command given" >&2; exit 2; }

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FRAMES="$REPO/doc/rust-rewrite/baseline/$LABEL"
SHOTS="$REPO/doc/rust-rewrite/screenshots/$LABEL"
SESS="sweep-$LABEL"

# tmux refuses long socket paths, so the server dir has to be short.
TMUXDIR="$(mktemp -d /tmp/fuisweepXXXX)"
ROOT="$(mktemp -d)"

cleanup() {
  tu kill --name "$SESS" >/dev/null 2>&1
  rm -rf "$TMUXDIR" "$ROOT"
}
trap cleanup EXIT

rm -rf "$FRAMES" "$SHOTS"
mkdir -p "$FRAMES" "$SHOTS" "$ROOT"/{home/.config/forestui,src,forest,bin}

# ---------------------------------------------------------------- fixture

for stub in vim mc claude; do
  printf '#!/bin/sh\necho "%s stub $*"\nexec sleep 600\n' "$stub" > "$ROOT/bin/$stub"
  chmod +x "$ROOT/bin/$stub"
done

cat > "$ROOT/home/.gitconfig" <<'EOF'
[user]
  name = Test
  email = test@example.com
[init]
  defaultBranch = main
EOF

for name in alpha beta; do
  d="$ROOT/src/$name"
  mkdir -p "$d"
  git -c init.defaultBranch=main init -q "$d"
  echo "# $name" > "$d/README.md"
  git -C "$d" add -A
  git -C "$d" -c user.name=Test -c user.email=t@e.com commit -qm "init $name"
  git -C "$d" branch -q feat/other
  git -C "$d" branch -q release-2
done
git -C "$ROOT/src/alpha" worktree add -q -b feat/wt-a "$ROOT/forest/alpha/wt-a"
REF="$(git -C "$ROOT/src/alpha" rev-parse --short HEAD)"

cat > "$ROOT/home/.config/forestui/settings.json" <<'EOF'
{"default_editor":"vim","default_terminal":"","branch_prefix":"feat/","theme":"system",
 "custom_buttons":[{"label":"Opus","prefix":"opus","command":"claude --model opus"}]}
EOF

cat > "$ROOT/forest/.forestui-config.json" <<EOF
{"repositories":[
 {"id":"11111111-1111-4111-8111-111111111111","name":"alpha","source_path":"$ROOT/src/alpha",
  "worktrees":[{"id":"22222222-2222-4222-8222-222222222222","name":"wt-a","branch":"feat/wt-a",
   "path":"$ROOT/forest/alpha/wt-a","is_archived":false,"sort_order":0,
   "last_modified":"2026-08-14T10:00:00Z","base_branch":"main","created_from_ref":"$REF"}]},
 {"id":"33333333-3333-4333-8333-333333333333","name":"beta","source_path":"$ROOT/src/beta","worktrees":[]}]}
EOF

# ---------------------------------------------------------------- helpers

press() { tu press --name "$SESS" "$@" >/dev/null 2>&1; }
type_()  { tu type  --name "$SESS" "$1" >/dev/null 2>&1; }
focus_app() { press Ctrl+B 0; sleep 0.7; }

# Wait until the screen shows <regex>, rather than sleeping a fixed amount.
# Textual repaints noticeably slower than ratatui, so a fixed sleep captures the
# two builds at different points in the same interaction.
await() { tu wait --name "$SESS" --text "$1" --timeout "${2:-12000}" >/dev/null 2>&1; }

# capture <UC-ID> <slug> — one PNG plus one normalised text frame.
#
# Volatile cells (relative times, commit hashes, temp paths, the tmux clock and
# the session's own dev-mode timestamp) are masked so two runs of the same build
# diff clean and two builds differ only where behaviour differs.
capture() {
  local id="$1" slug="$2"
  tu screenshot --name "$SESS" --png -o "$SHOTS/$id-$slug.png" >/dev/null 2>&1
  tu screenshot --name "$SESS" 2>/dev/null | ROOT="$ROOT" python3 -c '
import json, os, re, sys
root = os.environ["ROOT"]
text = json.load(sys.stdin)["content"]
text = text.replace(root, "<ROOT>")
text = re.sub(r"\b[0-9a-f]{7}\b", "<sha>", text)
text = re.sub(r"\(\d+ (seconds?|minutes?|hours?|days?) ago\)", "(<rel>)", text)
text = re.sub(r"\((an?|a) (second|minute|hour|day) ago\)", "(<rel>)", text)
text = re.sub(r"dev-\d{4}", "dev-<hhmm>", text)
text = re.sub(r"\d{2}:\d{2} \d{2}-\w{3}-\d{2}", "<clock>", text)
text = re.sub(r"\"[0-9a-f-]{8,}\"", "\"<host>\"", text)
print("\n".join(line.rstrip() for line in text.splitlines()))
' > "$FRAMES/$id-$slug.txt"
  printf '  %s %s\n' "$id" "$slug"
}

# ---------------------------------------------------------------- run

echo "sweeping $LABEL -> $FRAMES"

# The Python build's ensure_tmux re-execs the bare name `forestui`, so the
# command's own directory has to be on PATH or the tmux session dies instantly.
# The Rust build resolves itself via current_exe() and does not need this.
CMD_DIR="$(cd "$(dirname "${FUI_CMD[0]}")" && pwd)"

tu kill --name "$SESS" >/dev/null 2>&1
tu run --name "$SESS" --size 140x44 \
  --env TMUX_TMPDIR="$TMUXDIR" \
  --env HOME="$ROOT/home" \
  --env PATH="$CMD_DIR:$ROOT/bin:$(dirname "$(command -v tmux)"):/usr/bin:/bin:/usr/sbin:/sbin" \
  --env FORESTUI_NO_AUTO_UPDATE=1 \
  --cwd "$REPO" \
  -- env -u TMUX "${FUI_CMD[@]}" "$ROOT/forest" >/dev/null 2>&1
sleep 6

capture UC-53 boot-repository-detail

# The Textual tree needs one extra Down: the first press only highlights the
# root before selection follows the cursor.
press Down; sleep 1.2
if [ "$LABEL" = "python" ]; then press Down; sleep 1.2; fi
capture UC-54 worktree-detail

for spec in "a:UC-55:modal-add-repository:Repository Path" \
            "w:UC-56:modal-add-worktree:Worktree Name" \
            "s:UC-57:modal-settings:BRANCH PREFIX" \
            "d:UC-58:modal-confirm-delete:Permanently delete"; do
  IFS=: read -r key id slug expect <<< "$spec"
  focus_app; press "$key"
  await "$expect"; sleep 0.8
  capture "$id" "$slug"
  focus_app; press Escape; sleep 1.0
done

focus_app; press '?'; sleep 1.2
capture UC-59 help-notification

for spec in "e:UC-60:window-editor:edit:alpha" \
            "t:UC-61:window-terminal:term:alpha" \
            "o:UC-62:window-files:files:alpha" \
            "n:UC-63:window-claude:claude:alpha" \
            "y:UC-64:window-claude-yolo:yolo:alpha"; do
  IFS=: read -r key id slug w1 w2 <<< "$spec"
  focus_app; press "$key"
  await "$w1:$w2"
  sleep 1.0
  capture "$id" "$slug"
done

# Editor reuses its window; terminal always opens a new, uniquified one.
focus_app; press e
await "edit:alpha"; sleep 1.0
capture UC-65 window-editor-reused
focus_app; press t
await "term:alpha:wt-a:2"; sleep 1.0
capture UC-66 window-terminal-uniquified

focus_app; press A; sleep 1.2
capture UC-67 archived-section-toggle
focus_app; press h
await "Unarchive"; sleep 0.8
capture UC-68 worktree-archived
focus_app; press h
await "[^n]Archive"; sleep 0.8
capture UC-69 worktree-unarchived

focus_app; press r; sleep 1.5
capture UC-70 refresh

echo "done: $LABEL"
