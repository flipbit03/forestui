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
    """The tmux windows forestui opened, in order, and which one is active.

    Only the `<n>:<name>` entries are compared. The session-name prefix is
    deliberately excluded: a source build auto-enables dev mode and calls its
    own window `forestui-dev-<hhmm>` where a release calls it `forestui`, and
    that difference also shifts where tmux truncates the status bar. Comparing
    the raw line reported DIFF on every case, which hides the regressions this
    column exists to catch.
    """
    found = re.findall(r"\[forestui\S*[^\"]*", text)
    if not found:
        return {}
    line = re.sub(r"forestui-dev-<hhmm>", "forestui", found[-1].strip())
    entries = re.findall(r"\b(\d+):([\w:.\-/]+?)(\*?)(?=\s|$)", line)
    # tmux cuts the status bar with a trailing `>`, leaving the last entry a
    # fragment. Comparing a fragment against a whole name is a false alarm.
    if line.endswith(">") and entries:
        entries = entries[:-1]
    # Keyed by window index rather than position, because the two bars do not
    # truncate at the same point: a source build's window 0 is
    # `forestui-dev-<hhmm>` where a release's is `forestui`, nine columns
    # shorter, so the longer bar starts dropping windows off the left (tmux
    # marks it `<`) one window sooner. Comparing by position then reported every
    # window as renamed. Only indices both bars had room to print are compared.
    return {index: name + active for index, name, active in entries}

def phrases(text):
    out = set()
    for line in text.splitlines():
        stripped = "".join(" " if ch in BOX else ch for ch in line)
        for chunk in re.split(r"\s{2,}", stripped):
            phrase = " ".join(chunk.split())
            if len(phrase) > 2 and not set(phrase) <= set(". -"):
                out.add(phrase)
    return out

# Cases retired from the sweep whose frozen frames may survive on one side
# only (the python baselines are read-only history). One-sided by design,
# never a failure — in either argument order.
RETIRED = {"UC-59-help-notification"}

fail = 0
print(f"{'case':36} {'windows':>8} {f'only-{a}':>14} {f'only-{b}':>14}")
print("-" * 76)
names = sorted({p.name for p in da.glob("UC-*.txt")} | {p.name for p in db.glob("UC-*.txt")})
for name in names:
    fa, fb = da / name, db / name
    if fa.stem in RETIRED:
        print(f"{fa.stem:36} {'RETIRED':>8}")
        continue
    if not fa.exists() or not fb.exists():
        print(f"{fa.stem:36} {'MISSING':>8}")
        fail += 1
        continue
    ta, tb = fa.read_text(), fb.read_text()
    wa, wb = window_list(ta), window_list(tb)
    shared = wa.keys() & wb.keys()
    if not wa and not wb:
        verdict = "n/a"
    elif not shared:
        # Both bars printed windows but share no index — that is a real
        # divergence, not a truncation artifact.
        verdict = "n/a" if not (wa and wb) else "DIFF"
    else:
        verdict = "same" if all(wa[k] == wb[k] for k in shared) else "DIFF"
    if verdict == "DIFF":
        fail += 1
    pa, pb = phrases(ta), phrases(tb)
    print(f"{fa.stem:36} {verdict:>8} {len(pa - pb):>14} {len(pb - pa):>14}")

print()
print("window lists differ in", fail, "case(s)")
PY
