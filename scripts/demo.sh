#!/bin/sh
# Record the product demo: the 1M-line Find, multi-cursor, find in files, git and the terminal.
#
#   scripts/demo.sh prepare     build the app, make the 1M-line file, print the shot list
#   scripts/demo.sh record      run the scripted demo while the screen is recorded (about 75 s)
#   scripts/demo.sh play        run the scripted demo without recording (to rehearse)
#
# Needs (System Settings, Privacy & Security), for the app you run this from (Terminal, iTerm):
#   Accessibility        so the script can press keys in mide
#   Screen Recording     for `record`
# Every keystroke is only sent while mide is the frontmost app; otherwise the script stops.
# Click away or press Ctrl-C in the terminal to abort.
#
# Output: dist/demo/mide-demo.mov. Convert it with ffmpeg (brew install ffmpeg):
#   ffmpeg -i dist/demo/mide-demo.mov -vf "fps=30,scale=1600:-2" -c:v libx264 -crf 22 -pix_fmt yuv420p dist/demo/mide-demo.mp4
#   ffmpeg -i dist/demo/mide-demo.mov -vf "fps=12,scale=900:-1" dist/demo/mide-demo.gif
set -e
cd "$(dirname "$0")/.."
ROOT=$(pwd)
BIN="$ROOT/target/release/mide"
BIG=$(ls "$ROOT"/target/bench-data/big-1000000.rs 2>/dev/null | head -1 || true)
OUT="$ROOT/dist/demo"
MODE="${1:-prepare}"

prepare() {
    command -v cargo >/dev/null 2>&1 || . "$HOME/.cargo/env"
    cargo build --release -p mide -p mide-bench 2>&1 | tail -1
    [ -f "$ROOT/target/bench-data/big-1000000.rs" ] || target/release/mide-bench --filter "open file" >/dev/null
    BIG="$ROOT/target/bench-data/big-1000000.rs"
    echo "Ready: $BIG ($(du -h "$BIG" | cut -f1))"
    cat <<'SHOTS'

Shot list (what `record` does, about 75 seconds):
  1.  mide opens a 1,000,000-line file (about 35 MB)                         [0:00]
  2.  Cmd-F, type "function_999992(": it jumps to the last function at once [0:08]
  3.  Esc, then Cmd-D three times on a word and type: multi-cursor edit      [0:20]
  4.  Shift-Cmd-F: find in files across the project                          [0:32]
  5.  New window on the mide repo: Ctrl-Shift-G opens the git graph          [0:44]
  6.  Ctrl-` opens a terminal session at the bottom                          [0:60]
Tip: set the display to 1440x900 or smaller, hide the Dock, turn on Do Not Disturb.
SHOTS
}

# ---- keystrokes, only while mide is in front -------------------------------------------------
front() { osascript -e 'tell application "System Events" to get name of first application process whose frontmost is true' 2>/dev/null; }
guard() { [ "$(front)" = "mide" ] || { echo "mide is not the frontmost app; stopping." >&2; stop_recording; exit 1; }; }
key()   { guard; osascript -e "tell application \"System Events\" to keystroke \"$1\" using {$2}"; }
code()  { guard; osascript -e "tell application \"System Events\" to key code $1 using {$2}"; }
type_() { guard; osascript -e "tell application \"System Events\" to keystroke \"$1\""; }
slow()  { for c in $(printf '%s' "$1" | sed 's/./& /g'); do guard; osascript -e "tell application \"System Events\" to keystroke \"$c\""; sleep 0.09; done; }
focus() { osascript -e 'tell application "mide" to activate'; sleep 1; }

stop_recording() { [ -n "${REC_PID:-}" ] && kill -INT "$REC_PID" 2>/dev/null && wait "$REC_PID" 2>/dev/null || true; REC_PID=""; }

script() {
    BIG="$ROOT/target/bench-data/big-1000000.rs"
    "$BIN" "$BIG" & sleep 3; focus
    sleep 3
    # 2. Find at the end of the file
    key f "command down"; sleep 1
    slow "function_999992("; sleep 4
    code 53 ""; sleep 1                       # Esc closes the bar
    # 3. Multi-cursor: select a word at the top, add the next three, type over them
    code 126 "command down"; sleep 1.5          # Cmd-Up: top of the file
    key d "command down"; sleep .6; key d "command down"; sleep .6; key d "command down"; sleep .6; key d "command down"; sleep 1
    slow "renamed"; sleep 3
    key z "command down"; sleep 2              # undo
    # 4. Find in files
    key f "command down, shift down"; sleep 1
    slow "Matcher"; sleep 4
    code 53 ""; sleep 1
    # 5. Git graph in a window on the repo
    "$BIN" "$ROOT" & sleep 3; focus; sleep 2
    key g "control down, shift down"; sleep 6
    code 53 ""; sleep 1
    # 6. Terminal
    code 50 "control down"; sleep 4            # Ctrl-` (key code 50)
    slow "git log --oneline -5"; code 36 ""; sleep 4
    echo "Demo finished."
}

case "$MODE" in
    prepare) prepare ;;
    play)    prepare >/dev/null; script ;;
    record)
        prepare >/dev/null
        mkdir -p "$OUT"; rm -f "$OUT/mide-demo.mov"
        echo "Recording in 5 seconds. Leave the mouse alone; Ctrl-C to abort."; sleep 5
        screencapture -v -V85 "$OUT/mide-demo.mov" & REC_PID=$!
        trap 'stop_recording' INT TERM
        sleep 1; script
        stop_recording
        echo "Saved $OUT/mide-demo.mov"
        ;;
    *) echo "usage: scripts/demo.sh prepare|play|record" >&2; exit 2 ;;
esac
