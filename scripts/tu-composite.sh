#!/usr/bin/env bash
#
# tu-composite.sh — pair the screenshots of two sweeps side by side.
#
#   usage: scripts/tu-composite.sh [<label-a>] [<label-b>]   (default: rust python)
#
# tu-sweep.sh writes a PNG per use case under doc/rust-rewrite/screenshots/<label>/.
# The committed text frames catch structural drift, but colour, focus rings and
# selection highlights only exist in the pixels. This puts the two builds' frames
# for the same case in one image so a difference is visible rather than inferred.
#
# Output lands in doc/rust-rewrite/screenshots/composite/ (gitignored with the
# rest of the screenshots).

set -uo pipefail

A="${1:-rust}"
B="${2:-python}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SHOTS="$REPO/doc/rust-rewrite/screenshots"
OUT="$SHOTS/composite"

if [ ! -d "$SHOTS/$A" ] || [ ! -d "$SHOTS/$B" ]; then
  echo "missing screenshots: run scripts/tu-sweep.sh for both $A and $B" >&2
  exit 1
fi

mkdir -p "$OUT"

# Pillow via uv: forestui is a terminal application and carries no Python
# runtime of its own, so the dependency stays in the tool invocation rather than
# becoming something a contributor has to install first.
A="$A" B="$B" SHOTS="$SHOTS" OUT="$OUT" uv run --quiet --with pillow python3 - <<'PY'
import os
import pathlib
from PIL import Image, ImageDraw

a, b = os.environ["A"], os.environ["B"]
shots = pathlib.Path(os.environ["SHOTS"])
out = pathlib.Path(os.environ["OUT"])

BAR = 22          # height of the label strip above each frame
GAP = 12          # gutter between the two frames
BG = (24, 24, 26)
FG = (245, 245, 245)

written = 0
missing = []
for left in sorted((shots / a).glob("*.png")):
    right = shots / b / left.name
    if not right.exists():
        missing.append(left.name)
        continue

    with Image.open(left) as la, Image.open(right) as lb:
        ia, ib = la.convert("RGB"), lb.convert("RGB")
        # The two builds rarely produce the same pixel height; align on the
        # taller one so neither frame is cropped.
        height = max(ia.height, ib.height) + BAR
        canvas = Image.new("RGB", (ia.width + GAP + ib.width, height), BG)
        canvas.paste(ia, (0, BAR))
        canvas.paste(ib, (ia.width + GAP, BAR))

        draw = ImageDraw.Draw(canvas)
        draw.text((4, 6), a, fill=FG)
        draw.text((ia.width + GAP + 4, 6), b, fill=FG)
        canvas.save(out / left.name)
        written += 1

print(f"wrote {written} composite(s) to {out}")
for name in missing:
    print(f"  skipped {name}: no {b} capture")
PY
