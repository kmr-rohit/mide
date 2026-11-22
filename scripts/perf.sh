#!/bin/sh
# One command to check mide's performance.
#
#   scripts/perf.sh                  everything (engine benchmarks on a 1M-line file + the real app)
#   scripts/perf.sh --quick          100k-line fixture, about a minute
#   scripts/perf.sh --repo PATH      also time git operations on a big repository
#   scripts/perf.sh --json           also refresh the website's data (site-src/data/*.json)
#
# See docs/PERFORMANCE.md for what each number means and the budgets behind them.
set -e
cd "$(dirname "$0")/.."
command -v cargo >/dev/null 2>&1 || . "$HOME/.cargo/env"

QUICK=""; REPO=""; JSON=0
while [ $# -gt 0 ]; do
    case "$1" in
        --quick) QUICK="--quick" ;;
        --repo) REPO="--repo $2"; shift ;;
        --json) JSON=1 ;;
        *) echo "usage: scripts/perf.sh [--quick] [--repo PATH]" >&2; exit 2 ;;
    esac
    shift
done

cargo build --release -p mide -p mide-bench 2>&1 | tail -1
median() { sort -n | awk '{a[NR]=$1} END {if (NR==0) print "n/a"; else print (NR%2 ? a[(NR+1)/2] : (a[NR/2]+a[NR/2+1])/2)}'; }

echo
echo "=== 1/3  Engine benchmarks (buffer, search, syntax, fuzzy, files, terminal, git)"
if [ "$JSON" = 1 ]; then
    target/release/mide-bench $QUICK $REPO --json site-src/data/bench.json
else
    target/release/mide-bench $QUICK $REPO
fi

FILE=$(ls target/bench-data/big-*.rs | head -1)
echo
echo "=== 2/3  The real app on $(basename "$FILE") ($(du -h "$FILE" | cut -f1)): launch, typing, scrolling, far jump"
echo "    (5 launches; a window flashes up each time. Typing and scrolling are scripted.)"
STARTS=""; WINDOWS=""
for i in 1 2 3 4 5; do
    HOME_DIR=$(mktemp -d); LOG=$(mktemp)
    HOME="$HOME_DIR" MIDE_PERF=1 MIDE_PERF_TYPING=150 MIDE_PERF_EXIT=1 MIDE_PERF_JSON=/tmp/mide-perf-last.json target/release/mide "$FILE" > "$LOG" 2>&1 || true
    STARTS="$STARTS
$(sed -n 's/.*first editor paint: \([0-9.]*\) ms.*/\1/p' "$LOG")"
    WINDOWS="$WINDOWS
$(sed -n 's/.*window created: \([0-9.]*\) ms.*/\1/p' "$LOG")"
    cp "$LOG" /tmp/mide-perf-last.log
    rm -rf "$HOME_DIR" "$LOG"
done
echo "    window created        median $(echo "$WINDOWS" | sed '/^$/d' | median) ms   (GPUI + Metal start-up; goal < 150)"
echo "    first editor paint    median $(echo "$STARTS" | sed '/^$/d' | median) ms   (goal < 200)"
grep '^\[perf-summary\]' /tmp/mide-perf-last.log | sed 's/^\[perf-summary\] /    /'
SAMPLES=$(sed -n 's/.*typing: input->paint: n=\([0-9]*\).*/\1/p' /tmp/mide-perf-last.log | head -1)
if [ "${SAMPLES:-0}" -lt 120 ]; then
    echo "    WARNING: only ${SAMPLES:-0} of 150 typing samples were collected. macOS stops drawing a"
    echo "    window that is covered or off-screen, so keep the mide window visible (and the screen"
    echo "    awake) while this runs, then re-run."
fi
echo "    ('input->paint' includes waiting for the next screen refresh: ~8 ms average at 60 Hz."
echo "     'frame cpu' is the real work per frame; it must stay well under 16 ms.)"

echo
echo "=== 3/3  Idle cost with an editor and a terminal session open"
HOME_DIR=$(mktemp -d); PROJ=$(mktemp -d)
mkdir -p "$HOME_DIR/.config/mide"
printf '[project]\nopen_terminals = ["Shell"]\n' > "$HOME_DIR/.config/mide/config.toml"
echo 'fn main() {}' > "$PROJ/main.rs"
HOME="$HOME_DIR" target/release/mide "$PROJ/main.rs" > /dev/null 2>&1 &
sleep 6
PID=$(pgrep -nx mide || true)
if [ -n "$PID" ]; then
    CPU=$(top -l 3 -s 2 -pid "$PID" -stats cpu 2>/dev/null | grep -E '^[0-9]' | tail -1)
    RSS=$(ps -o rss= -p "$PID" | awk '{printf "%.0f", $1/1024}')
    echo "    CPU when idle: ${CPU:-n/a} %   (goal: 0.0)"
    echo "    memory:        ${RSS:-n/a} MB   (goal: < 200)"
    kill "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
    if [ "$JSON" = 1 ] && [ -f /tmp/mide-perf-last.json ]; then
        python3 - "$CPU" "$RSS" <<'PY'
import json, sys
d = json.load(open("/tmp/mide-perf-last.json"))
d["idle_cpu_percent"] = float(sys.argv[1] or 0)
d["memory_mb"] = float(sys.argv[2] or 0)
json.dump(d, open("site-src/data/app-perf.json", "w"), indent=2)
print("    wrote site-src/data/app-perf.json")
PY
    fi
else
    echo "    could not start the app"
fi
rm -rf "$HOME_DIR" "$PROJ"
echo
echo "Done. Details of the budgets and how to profile further: docs/PERFORMANCE.md"
