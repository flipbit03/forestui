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

# A `gh` that answers the four calls both builds make. Without it the GitHub
# section renders "No issues found" on both sides and the issue rows — card,
# title, labels, Create WT button — go uncompared, which is exactly how the
# session cards stayed unexamined for so long.
cat > "$ROOT/bin/gh" <<'GH'
#!/bin/sh
case "$1 $2" in
  "auth status") echo "Logged in to github.com as sweep-user"; exit 0 ;;
  "api user")    echo "sweep-user"; exit 0 ;;
  "repo view")   echo '{"owner":{"login":"sweep-user"},"name":"alpha"}'; exit 0 ;;
  "issue list")
    # Only the --assignee query returns rows; the --author one returns none, so
    # the de-duplication path is exercised too.
    case "$*" in
      *--assignee*)
        cat <<'JSON'
[{"number":326,"title":"Fansly conversation discovery has no backoff","state":"OPEN",
  "url":"https://example.invalid/326","createdAt":"2026-08-01T10:00:00Z",
  "updatedAt":"2026-08-02T10:00:00Z","author":{"login":"sweep-user"},
  "assignees":[{"login":"sweep-user"}],"labels":[{"name":"bug","color":"ff0000"}]},
 {"number":298,"title":"Proactive metrics: coalesced FIRE rows overcount","state":"OPEN",
  "url":"https://example.invalid/298","createdAt":"2026-07-20T10:00:00Z",
  "updatedAt":"2026-07-21T10:00:00Z","author":{"login":"sweep-user"},
  "assignees":[],"labels":[{"name":"metrics","color":"00ff00"},{"name":"p2","color":"0000ff"}]}]
JSON
        ;;
      *) echo "[]" ;;
    esac
    exit 0 ;;
esac
exit 1
GH
chmod +x "$ROOT/bin/gh"

cat > "$ROOT/home/.gitconfig" <<'EOF'
[user]
  name = Test
  email = test@example.com
[init]
  defaultBranch = main
EOF

for name in alpha beta gamma; do
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

# gamma is deliberately NOT in the config, and owns a worktree OUTSIDE the
# forest directory — that is what "Import existing worktrees" has to discover.
git -C "$ROOT/src/gamma" worktree add -q -b feat/imported "$ROOT/outside/gamma-imported"

# Claude sessions, so the RECENT SESSIONS cards actually render. Without these
# both builds only ever show "No sessions found", and that whole section goes
# uncompared.
seed_sessions() {
  local target="$1" count="$2"
  # The app canonicalises before building the folder name, and on macOS that
  # turns /var/... into /private/var/..., so seed against the resolved path.
  local resolved folder
  resolved="$(cd "$target" && pwd -P)"
  folder="$(printf '%s' "$resolved" | tr '/' '-')"
  local dir="$ROOT/home/.claude/projects/$folder"
  mkdir -p "$dir"
  for i in $(seq 1 "$count"); do
    cat > "$dir/session-$i.jsonl" <<SESSION
{"type":"user","timestamp":"2026-08-0${i}T10:00:00Z","message":{"content":"session $i: refactor the detail pane so the cards line up"}}
{"type":"assistant","timestamp":"2026-08-0${i}T10:00:05Z"}
{"type":"user","timestamp":"2026-08-0${i}T10:01:00Z","message":{"content":"and then check it against the python build"}}
SESSION
  done
}
# Repository only. Seeding the worktree too pushes Textual's RENAME inputs off
# a 70-row screen, and the rename flow then clicks on nothing.
seed_sessions "$ROOT/src/alpha" 3

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
# `--` matters: text starting with a dash is otherwise parsed as a flag,
# which silently types nothing at all.
type_()  { tu type  --name "$SESS" -- "$1" >/dev/null 2>&1; }
focus_app() { press Ctrl+B 0; sleep 0.7; }

# Wait until the screen shows <regex>, rather than sleeping a fixed amount.
# Textual repaints noticeably slower than ratatui, so a fixed sleep captures the
# two builds at different points in the same interaction.
# await <regex> [timeout-ms] — block until the screen shows something.
#
# A timeout is reported rather than swallowed. It used to return quietly and
# the caller captured anyway, so a case that never reached its state still
# wrote a frame — and that frame diffed clean against a baseline captured the
# same way, which is a green test for a screen nobody ever saw.
await() {
  if ! tu wait --name "$SESS" --text "$1" --timeout "${2:-12000}" >/dev/null 2>&1; then
    printf '  WAIT-TIMEOUT  %s\n' "$1" >&2
    WAIT_TIMEOUTS=$((WAIT_TIMEOUTS + 1))
  fi
}
WAIT_TIMEOUTS=0

# capture <UC-ID> <slug> [session] — one PNG plus one normalised text frame.
#
# Volatile cells (relative times, commit hashes, temp paths, the tmux clock and
# the session's own dev-mode timestamp) are masked so two runs of the same build
# diff clean and two builds differ only where behaviour differs.
capture() {
  local id="$1" slug="$2" sess="${3:-$SESS}"
  tu screenshot --name "$sess" --png -o "$SHOTS/$id-$slug.png" >/dev/null 2>&1
  tu screenshot --name "$sess" 2>/dev/null \
    | ROOT="$ROOT" HOST="$(hostname)" HOST_SHORT="$(hostname -s)" WHO="$(id -un)" python3 -c '
import json, os, re, sys
root = os.environ["ROOT"]
text = json.load(sys.stdin)["content"]
text = text.replace(root, "<ROOT>")
text = re.sub(r"\b[0-9a-f]{7}\b", "<sha>", text)
text = re.sub(r"\(\d+ (seconds?|minutes?|hours?|days?) ago\)", "(<rel>)", text)
text = re.sub(r"\((an?|a) (second|minute|hour|day) ago\)", "(<rel>)", text)
text = re.sub(r"dev-\d{4}", "dev-<hhmm>", text)
# Grouped sessions are named "forestui-<forest>-<pid>", so the pid would
# otherwise make every run dirty the committed baseline.
text = re.sub(r"(forestui-[a-z0-9-]+?)-\d+\b", r"\1-<pid>", text)
text = re.sub(r"\d{2}:\d{2} \d{2}-\w{3}-\d{2}", "<clock>", text)
# tmux prints the machine name in its status bar and the shell prompt inside a
# terminal window prints `user@host`. These frames are committed and travel to a
# public PR, so neither the machine nor the account reaches the baseline. Longest
# first: the short name is a prefix of the FQDN.
text = text.replace(os.environ["HOST"], "<host>")
text = text.replace(os.environ["HOST_SHORT"], "<host>")
text = text.replace(os.environ["WHO"], "<user>")
text = re.sub(r"\"[0-9a-f-]{8,}\"", "\"<host>\"", text)
# The Rust build runs from source at 0.0.0 while the Python build is an
# installed release; masking the version keeps the header comparable.
text = re.sub(r"forestui v\S+", "forestui v<version>", text)
print("\n".join(line.rstrip() for line in text.splitlines()))
' > "$FRAMES/$id-$slug.txt"
  printf '  %s %s\n' "$id" "$slug"
}

# assert <UC-ID> <description> <expected> <actual>
#
# Screens prove what the user sees; these prove what actually happened on disk.
# Both builds write the same file, so the outcomes diff as easily as the frames.
ASSERTIONS="$FRAMES/ASSERTIONS.txt"
: > "$ASSERTIONS"
assert() {
  local id="$1" what="$2" expected="$3" actual="$4" verdict=FAIL
  [ "$expected" = "$actual" ] && verdict=PASS
  printf '%-4s %-9s %-46s expected=%-28s actual=%s\n' \
    "$verdict" "$id" "$what" "$expected" "$actual" >> "$ASSERTIONS"
  [ "$verdict" = PASS ] || printf '    !! %s %s: expected %s, got %s\n' \
    "$id" "$what" "$expected" "$actual" >&2
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

# UC-95/96: the fixture must actually exercise every section, and the capture
# must be tall enough to show it. A section that falls back to its empty state —
# or scrolls off the frame — is a section nobody is comparing. The session cards
# went unexamined that way for days, and the issue rows right behind them.
tu resize --name "$SESS" 140x140 >/dev/null 2>&1
sleep 2.0
capture UC-96 repository-pane-full-height
tall="$FRAMES/UC-96-repository-pane-full-height.txt"

has_in_tall() { grep -qE "$1" "$tall" 2>/dev/null && echo yes || echo no; }
assert UC-96 sessions-rendered "yes" "$(has_in_tall 'msgs')"
assert UC-96 issues-rendered   "yes" "$(has_in_tall '#326|#298')"
assert UC-96 no-empty-sections "yes" \
  "$(grep -qE 'No sessions found|No issues found|No repositories' "$tall" && echo no || echo yes)"

tu resize --name "$SESS" 140x44 >/dev/null 2>&1
sleep 1.5

# UC-90: mouse reporting must be on, in any-motion mode, on both builds.
#
# This assertion used to demand the opposite of the Rust build — motion
# reporting was treated as a defect there, because every report woke the loop
# and repainted and that read as flicker. But `?1003h` is the only way a
# terminal reports a bare pointer move, so refusing it makes hover impossible
# rather than merely unstyled, and the stylesheet this port follows has 25
# `:hover` rules. The repaint is now gated on the hovered target *changing*, so
# motion is cheap and both builds legitimately want it on.
mouse_mode="$(tu mouse state --name "$SESS" 2>/dev/null | python3 -c "
import json,sys
try:
    s=json.load(sys.stdin)
    print(('on' if s.get('enabled') else 'off') + ':' + str(s.get('mode')))
except Exception:
    print('unknown')")"
capture UC-90 mouse-reporting-mode
assert UC-90 mouse-enabled "on" "${mouse_mode%%:*}"
assert UC-90 any-motion-for-hover "yes" \
  "$([ "${mouse_mode##*:}" = "AnyMotion" ] && echo yes || echo no)"

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
# Nothing new appears — that is the whole point of the case — so there is no
# screen condition to wait on. `await "edit:alpha"` looked like one but matched
# the status bar UC-60 already left behind.
sleep 1.5
capture UC-65 window-editor-reused
focus_app; press t
await "term:alpha:wt-a:2"; sleep 1.0
capture UC-66 window-terminal-uniquified

focus_app; press A; sleep 1.2
capture UC-67 archived-section-toggle
focus_app; press h
await "│ Unarchive │"; sleep 0.8
capture UC-68 worktree-archived
focus_app; press h
await "│ Archive │"; sleep 0.8
capture UC-69 worktree-unarchived

focus_app; press r; sleep 1.5
capture UC-70 refresh

# ------------------------------------------------- flows (UC-78 .. UC-82)
#
# Multi-step flows. Unlike the single-key sweep these also assert on disk, in
# ASSERTIONS.txt, because the interesting part of a rename or an import is what
# it did to the worktree and the config — not only what the screen said.
#
# The two builds reach the same controls differently: the Rust detail pane is a
# keyboard focus ring, the Textual one is a scrollable pane of mouse-first
# widgets (ratatui here ignores mouse entirely). Each flow therefore branches on
# $LABEL for the *interaction* while asserting the same *outcome*.

cfg() { python3 -c "
import json,sys
d=json.load(open('$ROOT/forest/.forestui-config.json'))
print(eval(sys.argv[1], {'d': d}))" "$1" 2>/dev/null || echo "<unreadable>"; }

# A taller terminal so the RENAME section is on screen in both builds; the
# Textual layout needs roughly 60 rows to reach it without scrolling.
tu resize --name "$SESS" 140x70 >/dev/null 2>&1; sleep 1.5
focus_app

# --- UC-78 / UC-79: rename the worktree, then its branch ---------------------
#
# Walk to the top first. Earlier phases moved the cursor, and a relative Down
# from wherever it happened to be landed on the wrong row — which silently made
# this flow rename nothing at all.
for _ in 1 2 3 4 5; do press Up; sleep 0.2; done
sleep 1.0
press Down; sleep 1.5
await "WORKTREE"
capture UC-78 rename-before

if [ "$LABEL" = "rust" ]; then
  # Click the field rather than counting Tab/Down steps: the focus ring's length
  # depends on custom buttons and sessions, and after an earlier action focus may
  # already be in the detail pane, so a blind Tab toggles the wrong way.
  tu mouse click --name "$SESS" --on-text "Worktree name" >/dev/null 2>&1
  sleep 0.8; press End; type_ "-renamed"; sleep 0.8
else
  # Textual: click into the pre-filled Input, then append. The double border
  # (`│  │`) is what distinguishes the input box from the label of the same name
  # elsewhere on screen, so target that rather than an occurrence count.
  tu mouse click --name "$SESS" --on-regex "│  │  wt-a" >/dev/null 2>&1
  sleep 0.8; press End; type_ "-renamed"; sleep 0.8
fi
capture UC-78 rename-typed
press Enter; sleep 3.5
capture UC-78 rename-after

assert UC-78 wt-name "wt-a-renamed" "$(cfg "d['repositories'][0]['worktrees'][0]['name']")"
assert UC-78 wt-dir-exists "yes" "$([ -d "$ROOT/forest/alpha/wt-a-renamed" ] && echo yes || echo no)"
assert UC-78 old-dir-gone "yes" "$([ -d "$ROOT/forest/alpha/wt-a" ] && echo no || echo yes)"
assert UC-78 git-worktree-ok "yes" \
  "$(git -C "$ROOT/forest/alpha/wt-a-renamed" rev-parse --git-dir >/dev/null 2>&1 && echo yes || echo no)"

focus_app
if [ "$LABEL" = "rust" ]; then
  tu mouse click --name "$SESS" --on-text "Branch name" >/dev/null 2>&1
  sleep 0.8; press End; type_ "-v2"; sleep 0.8
else
  tu mouse click --name "$SESS" --on-regex "│  │  feat/wt-a" >/dev/null 2>&1
  sleep 0.8; press End; type_ "-v2"; sleep 0.8
fi
press Enter; sleep 3.5
capture UC-79 rename-branch-after

assert UC-79 branch-in-config "feat/wt-a-v2" "$(cfg "d['repositories'][0]['worktrees'][0]['branch']")"
assert UC-79 branch-checked-out "feat/wt-a-v2" \
  "$(git -C "$ROOT/forest/alpha/wt-a-renamed" branch --show-current 2>/dev/null || echo '<none>')"

# --- UC-80: add a repository with "Import existing worktrees" ----------------
focus_app
press a; await "Repository Path"; sleep 0.8
type_ "$ROOT/src/gamma"; sleep 1.0
if [ "$LABEL" = "rust" ]; then
  press Tab; sleep 0.4          # focus 1 = checkbox
  press Space; sleep 0.4
else
  tu mouse click --name "$SESS" --on-text "Import existing worktrees" >/dev/null 2>&1
  sleep 0.6
fi
capture UC-80 import-checked
if [ "$LABEL" = "rust" ]; then
  press Tab; sleep 0.4; press Enter
else
  tu mouse click --name "$SESS" --on-regex "│ Add Repository │" >/dev/null 2>&1
fi
sleep 4
capture UC-80 import-after

assert UC-80 gamma-tracked "gamma" "$(cfg "[r['name'] for r in d['repositories']][-1]")"
assert UC-80 imported-worktree "['feat/imported']" \
  "$(cfg "[w['branch'] for w in d['repositories'][-1]['worktrees']]")"

tu resize --name "$SESS" 140x44 >/dev/null 2>&1; sleep 1

# --- UC-81: a second terminal joins as a grouped session ---------------------
#
# Same TMUX_TMPDIR means the same tmux server, so the second instance joins the
# window group instead of starting its own. Both terminals must see the same
# windows while navigating them independently.
SESS2="$SESS-b"
tu kill --name "$SESS2" >/dev/null 2>&1
tu run --name "$SESS2" --size 140x44 \
  --env TMUX_TMPDIR="$TMUXDIR" \
  --env HOME="$ROOT/home" \
  --env PATH="$CMD_DIR:$ROOT/bin:$(dirname "$(command -v tmux)"):/usr/bin:/bin:/usr/sbin:/sbin" \
  --env FORESTUI_NO_AUTO_UPDATE=1 \
  --cwd "$REPO" \
  -- env -u TMUX "${FUI_CMD[@]}" "$ROOT/forest" >/dev/null 2>&1
sleep 7
capture UC-81 grouped-terminal-b "$SESS2"
capture UC-81 grouped-terminal-a "$SESS"

winlist() {
  tu screenshot --name "$1" 2>/dev/null | python3 -c "
import json,sys,re
line=json.load(sys.stdin)['content'].splitlines()[-1]
print(len(re.findall(r'\s\d+:', line)))"
}
assert UC-81 both-see-same-window-count "$(winlist "$SESS")" "$(winlist "$SESS2")"

# Terminal B switches window; terminal A must not follow.
a_before="$(tu screenshot --name "$SESS" 2>/dev/null | python3 -c "
import json,sys,re
line=json.load(sys.stdin)['content'].splitlines()[-1]
m=re.search(r'(\d+):\S*\*', line); print(m.group(1) if m else '?')")"
tu press --name "$SESS2" Ctrl+B n >/dev/null 2>&1; sleep 2
a_after="$(tu screenshot --name "$SESS" 2>/dev/null | python3 -c "
import json,sys,re
line=json.load(sys.stdin)['content'].splitlines()[-1]
m=re.search(r'(\d+):\S*\*', line); print(m.group(1) if m else '?')")"
capture UC-81 grouped-b-switched "$SESS2"
capture UC-81 grouped-a-unmoved "$SESS"
assert UC-81 terminal-a-stayed-put "$a_before" "$a_after"

tu kill --name "$SESS2" >/dev/null 2>&1

# --- UC-82: detach, then relaunch and reattach -------------------------------
#
# Launching through a login shell so the shell survives the detach; if forestui
# were the tu process itself, detaching would kill the session outright.
SESS3="$SESS-c"
tu kill --name "$SESS3" >/dev/null 2>&1
tu run --name "$SESS3" --size 140x44 \
  --env TMUX_TMPDIR="$TMUXDIR" \
  --env HOME="$ROOT/home" \
  --env PATH="$CMD_DIR:$ROOT/bin:$(dirname "$(command -v tmux)"):/usr/bin:/bin:/usr/sbin:/sbin" \
  --env FORESTUI_NO_AUTO_UPDATE=1 \
  --cwd "$REPO" \
  -- env -u TMUX bash --noprofile --norc >/dev/null 2>&1
sleep 2
tu type --name "$SESS3" "${FUI_CMD[*]} $ROOT/forest" >/dev/null 2>&1
tu press --name "$SESS3" Enter >/dev/null 2>&1
tu wait --name "$SESS3" --text "MAIN REPOSITORY|WORKTREE|No repositories" --timeout 15000 >/dev/null 2>&1
sleep 2
capture UC-82 reattach-first-run "$SESS3"

tu press --name "$SESS3" Ctrl+B d >/dev/null 2>&1; sleep 2
capture UC-82 reattach-detached "$SESS3"

tu type --name "$SESS3" "${FUI_CMD[*]} $ROOT/forest" >/dev/null 2>&1
tu press --name "$SESS3" Enter >/dev/null 2>&1
tu wait --name "$SESS3" --text "MAIN REPOSITORY|WORKTREE|No repositories" --timeout 15000 >/dev/null 2>&1
sleep 2
capture UC-82 reattach-second-run "$SESS3"

# Reattaching must not leave a second forestui window behind.
fui_windows="$(tu screenshot --name "$SESS3" 2>/dev/null | python3 -c "
import json,sys,re
line=json.load(sys.stdin)['content'].splitlines()[-1]
print(len(re.findall(r'forestui(-dev-\d+)?[-*]?\s', line)))")"
assert UC-82 single-forestui-window "1" "$fui_windows"

tu kill --name "$SESS3" >/dev/null 2>&1

# --- UC-83 .. UC-86: mouse ---------------------------------------------------
#
# Textual is mouse-first; ratatui has no built-in notion of a clickable widget,
# so the Rust build records a rectangle per control each frame and resolves
# clicks against that. Both builds are driven the same way here, by clicking on
# label text, which is what a user actually does.

screen_has() {
  tu screenshot --name "$SESS" 2>/dev/null | python3 -c "
import json,re,sys
print('yes' if re.search(sys.argv[1], json.load(sys.stdin)['content']) else 'no')" "$1"
}
click_text() { tu mouse click --name "$SESS" --on-text "$1" >/dev/null 2>&1; }

focus_app
click_text "beta"; sleep 2
capture UC-83 click-sidebar-row
assert UC-83 detail-follows-click "yes" "$(screen_has 'Repository: beta')"

focus_app
click_text "Terminal"; sleep 3
capture UC-84 click-detail-control
assert UC-84 terminal-window-opened "yes" "$(screen_has 'term:beta')"

focus_app
press a; await "Repository Path"; sleep 0.8
click_text "Cancel"; sleep 1.5
capture UC-86 click-modal-cancel
assert UC-86 modal-dismissed-by-click "no" "$(screen_has 'Repository Path')"

echo "done: $LABEL"
if [ "$WAIT_TIMEOUTS" -gt 0 ]; then
  # Loud on purpose: every timed-out wait captured a frame of whatever was on
  # screen instead, so the frames below are not all what they claim to be.
  echo "WARNING: $WAIT_TIMEOUTS wait(s) timed out — those captures are suspect"
fi
echo "assertions:"
sed 's/^/  /' "$ASSERTIONS"
