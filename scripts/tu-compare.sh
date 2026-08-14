#!/usr/bin/env bash
#
# tu-compare.sh — compare two sweeps captured by tu-sweep.sh.
#
#   usage: scripts/tu-compare.sh [<label-a>] [<label-b>]   (default: rust python)
#
# Reports, per use case, whether the tmux window list matches and which
# meaningful phrases each build shows that the other does not. Box-drawing
# chrome is stripped, because the two builds draw different chrome by design —
# what has to agree is the text.

set -uo pipefail

A="${1:-rust}"
B="${2:-python}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

A="$A" B="$B" BASE="$REPO/doc/rust-rewrite/baseline" python3 - <<'PY'
import os, pathlib, re, sys

base = pathlib.Path(os.environ["BASE"])
a, b = os.environ["A"], os.environ["B"]
da, db = base / a, base / b
if not da.is_dir() or not db.is_dir():
    sys.exit(f"missing baseline: run scripts/tu-sweep.sh for both {a} and {b}")

BOX = "│┌┐└┘─▊▔▁▎▐▏▃▅▂█▌▄╭╮╯╰┏┓┗┛━┃⭘"

def window_list(text):
    found = re.findall(r"\[forestui\S*[^\"]*", text)
    return found[-1].strip() if found else ""

def phrases(text):
    out = set()
    for line in text.splitlines():
        stripped = "".join(" " if ch in BOX else ch for ch in line)
        for chunk in re.split(r"\s{2,}", stripped):
            phrase = " ".join(chunk.split())
            if len(phrase) > 2 and not set(phrase) <= set(". -"):
                out.add(phrase)
    return out

fail = 0
print(f"{'case':36} {'windows':>8} {f'only-{a}':>14} {f'only-{b}':>14}")
print("-" * 76)
for fa in sorted(da.glob("UC-*.txt")):
    fb = db / fa.name
    if not fb.exists():
        print(f"{fa.stem:36} {'MISSING':>8}")
        fail += 1
        continue
    ta, tb = fa.read_text(), fb.read_text()
    wa, wb = window_list(ta), window_list(tb)
    verdict = "n/a" if not wa and not wb else ("same" if wa == wb else "DIFF")
    if verdict == "DIFF":
        fail += 1
    pa, pb = phrases(ta), phrases(tb)
    print(f"{fa.stem:36} {verdict:>8} {len(pa - pb):>14} {len(pb - pa):>14}")

print()
print("window lists differ in", fail, "case(s)")
PY
