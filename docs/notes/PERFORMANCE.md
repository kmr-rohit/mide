# Testing mide's performance

## The short version

```sh
scripts/perf.sh --quick                 # about a minute
scripts/perf.sh                         # full: 1M-line (~50 MB) file
scripts/perf.sh --repo ~/code/big-repo  # also times git on a big repository
```

Always use the release build (the script does). Debug numbers are meaningless: 10 to 50x slower.
Save a run to compare later: `scripts/perf.sh --quick > perf-$(date +%F).txt`.

The script has three parts.

### 1. Engine benchmarks (`crates/bench`)

`cargo run --release -p mide-bench` times the pieces the UI is built on, with a **budget** per
line and a pass/fail column (`--strict` exits 1 if any budget is missed; `--filter search`
runs a subset; `--lines N` changes the fixture size).

| Area | What is timed | Why the budget |
|---|---|---|
| core | open a 1M-line file; read the 60 visible lines; type a character; 100 cursors; undo | a keystroke must cost far less than a 16 ms frame |
| search | literal / regex / no-match full scans, find-next, replace-all | a find-as-you-type pass over a huge file should stay under ~0.3 s |
| syntax | first window, re-highlight after an edit, **far jump**, throughput | a window is 60 lines: it must fit in a few ms; a far jump must never freeze |
| fuzzy | ⌘P over 100k paths | per-keystroke: about 12 ms or less |
| files | list 20k files, open the tree | ⌘P index and sidebar |
| terminal | take in ~8 MB from a pty, emulator speed, per-frame snapshot | `cat` of a big log; the snapshot is taken every frame |
| git | log + graph, status, diff (needs `--repo`) | the git window must open quickly on a real repo |

> Terminal throughput note: a program that writes one line per syscall (`seq`, a shell loop)
> is limited by the OS pty to ~2 MB/s on macOS **no matter which terminal reads it**. The
> benchmark uses a producer that writes in blocks (`yes | head`), which measures mide.

### 2. The real app, scripted (`MIDE_PERF_TYPING`)

The app can drive itself, so typing latency is measured inside the real UI with no human:

```sh
MIDE_PERF=1 MIDE_PERF_TYPING=200 MIDE_PERF_EXIT=1 target/release/mide some/big/file.rs
```

It prints start-up marks, then types 200 characters, scrolls 200 frames, and jumps to the end
of the file, reporting p50/p95/max for each. Reading the numbers:

* **`frame cpu`**: the actual work per frame (layout + paint calls). Keep p95 well under 16 ms.
  Typical today: 4-5 ms.
* **`input->paint`**: includes waiting for the next screen refresh (about 8 ms on average at
  60 Hz, about 4 ms on a 120 Hz ProMotion display), so ~9 ms p50 is already near the floor.
* It measures up to the end of the paint call, not GPU presentation. For that, see Instruments below.

For ad-hoc checking, `MIDE_PERF=1` alone prints a line for every keystroke you type yourself,
plus any frame slower than 8 ms. `MIDE_PERF_TYPING` needs the window to stay **visible**:
macOS stops drawing covered windows, and the script warns when it gets too few samples.

Start-up marks (`[perf] ... ms since launch`): `window created` (GPUI + Metal, about 150 ms,
outside mide's control), then the workspace steps, then `first editor paint`.

### 3. Idle cost

With an editor and a terminal session open, mide should use **0.0 % CPU** and well under
200 MB. Anything else means a busy loop or a leak. (An earlier version polled the terminal
every 12 ms and burned about 8 %; the benchmark made that easy to catch.)

## Manual scenarios worth trying

* **Huge file**: open `target/bench-data/big-1000000.rs` (the script creates it); hold the scroll wheel,
  hold ⌘↓/⌘↑ and PageDown, then select-all and copy. (There is no scrollbar yet, see the review.)
* **One giant line**: a minified JSON/JS file. Today ~47 ms per keystroke on a 3 MB line (see the review).
* **Many tabs**: open 200 files via ⌘P, switch with ⌃Tab.
* **Noisy terminal**: `yes | head -n 2000000`, then resize the window while it runs; watch idle CPU after.
* **Soak test for leaks**: open and close 100 tabs / 50 terminal sessions; the memory line in
  `scripts/perf.sh` (or Activity Monitor) should return near its starting value.
* **Big repository**: `scripts/perf.sh --repo <path>`; then open the git window (⌃⇧G) and select
  a big commit and a dirty working copy.

## Profiling when a number is bad

* **CPU**: Instruments ▸ *Time Profiler*:
  `xcrun xctrace record --template 'Time Profiler' --launch target/release/mide -- file.rs`
  then open the `.trace`. Build with symbols for readable stacks:
  `CARGO_PROFILE_RELEASE_DEBUG=true cargo build --release -p mide`.
* **GPU / frame pacing / dropped frames**: Instruments ▸ *Metal System Trace*.
* **Memory**: `leaks $(pgrep -x mide)`, `vmmap -summary $(pgrep -x mide)`, or Instruments ▸ *Allocations*.
* **A hang** (beachball): `sample $(pgrep -x mide) 5 -file /tmp/hang.txt` while it is stuck; the busiest
  stack shows what the UI thread is doing.
* **Which phase is slow at start-up**: the `[perf]` marks, or add `perf::mark("...")` anywhere.

## Adding a benchmark

Add a `rep.record("name", median_ms, p95_ms, budget_ms)` call in `crates/bench/src/main.rs`
using `measure(iters, || ...)`. Pick the budget from what the user feels (a keystroke: under a
millisecond or two; something the user triggers: a few hundred ms at most; never a freeze), not
from what the code happens to do today.

## Where things stand (this machine, release build)

| | |
|---|---|
| Open a 1M-line / 33 MB file | about 10-40 ms |
| Type a character | under 1 µs in the buffer; 4-5 ms of frame CPU, 9 ms p50 input to paint |
| Scroll 3 lines/frame | 3-5 ms frame CPU |
| Jump to line 100,000 (cold) | 12 ms (was **3.8 s** before the fix) |
| Find over 1M lines | 27 ms (100k lines) to 0.15-0.29 s (1M lines) |
| ⌘P over 100k files | 3.5-8 ms |
| Start-up to first editor paint | about 180 ms (was 334 ms before the font fix) |
| Idle | 0.0 % CPU, about 110 MB |
