# Product review

An honest audit of mide as of this build. Everything marked **measured** or **tested** was
verified by running it; everything else is from reading the code. I could not look at the
window myself while building, so **no screen has been visually reviewed by me**: the
look-and-feel items below come from your screenshots and from the code, not from eyes on pixels.

## What is solid

* **Speed.** A 33 MB / 1M-line file opens in tens of ms and edits in microseconds; frame CPU while
  typing is 4-5 ms; idle is 0.0 % CPU. (Measured: `docs/PERFORMANCE.md`.)
* **Engine quality.** Rope buffer, multi-cursor, grouped undo, find/replace with regex captures, line
  operations, a real terminal emulator, incremental highlighting, a git layer tested against real
  repositories including push/pull. About 150 automated tests, 30 benchmarks with budgets.
* **Safety nets.** Atomic saves, CRLF/BOM preserved, never overwrites a non-UTF-8 file, unsaved-file
  prompts, terminal sessions that cannot hang the UI.

## Found and fixed during this review

| Finding | How I found it | Fix |
|---|---|---|
| **Opening a folder ran commands from its `.mide/config.toml`** (arbitrary code execution from any cloned repo) | Reproduced: a repo's config created a file on open | **Workspace trust**: project terminal profiles and `open_terminals` are ignored until you run "Workspace: Trust This Folder". Verified closed. |
| Jumping far into a big file froze the UI for **3.8 s** (highlighter parsed every line above) | Benchmark: cold jump to line 100,000 | Highlight from a 250-line warm-up for far windows: **12 ms**. Colors can be slightly off if a block comment opened further up. |
| 140 ms of start-up spent listing every installed font | Start-up timing marks | Probe the one family needed: first paint 334 to **182 ms** |
| Closing a terminal whose program ignores hang-up froze the app | Reproduced with a test | Non-blocking close with SIGKILL fallback (earlier) |
| Idle terminal burned ~8 % CPU | Idle measurement | Event-driven wake-up: 0.0 % (earlier) |
| `⌘⇧G` could not be both "Find previous" and "Git history" | Shortcut audit | Git history moved to **⌃⇧G**; ⌘⇧G is Find Previous (macOS standard) |
| "Plain Text" got LaTeX's `%` comment marker | Unit test | Fixed |

## Gaps, most important first

### P0: can lose or corrupt your work

1. **Files changed on disk are not noticed while open.** The watcher exists but open buffers ignore
   it. If a formatter, `git checkout`, or Claude/Copilot edits a file you have open, the editor still
   shows the old text, and saving overwrites the new content silently. *Fix*: on change, reload clean
   buffers; for dirty ones, show a "changed on disk" bar (reload / keep mine / diff).
2. **No crash recovery or session restore.** Tabs, cursor positions and unsaved edits vanish if the
   app quits or crashes. *Fix*: restore tabs and layout on launch; periodically back up dirty buffers.
3. **No "Save As", no new/untitled file.** You cannot create a file from inside mide. The sidebar has
   no new file/folder, rename, delete or reveal-in-Finder.
4. **Opening a binary file** loads it as lossy text (saving is refused, but a 500 MB binary is still read).
   Needs a "binary file" guard. Encodings other than UTF-8 are not supported.

### P1: expected in a daily editor

5. **Find in files (⇧⌘F)** and project-wide replace. Single-file find/replace now exists.
6. **Indentation detection.** Indent is always 4 spaces and the status bar says "Spaces: 4" even for a
   2-space YAML file (your own screenshots use 2). No tabs-vs-spaces detection, no auto-indent after
   `{` or `:`, no bracket matching or auto-closing pairs.
7. **No scrollbar** (editor or sidebar) and **no minimap** (your Sublime has both); no soft wrap; no
   code folding; no split editor panes; no Go to Symbol (⌘R) / Go to Definition / completion (no LSP).
8. **Tab bar**: no overflow handling (many tabs clip), no drag to reorder, no preview tabs, no
   "Open Recent" menu or recent-files list.
9. **Sidebar tree is not virtualized**: every expanded row is laid out each frame, so expanding a huge
   folder (`node_modules`) will be slow. *Fix*: `uniform_list`, as the git list already does.
10. **Settings are TOML only.** No settings UI; keybindings are hard-coded (no user keymap file).
11. **No right-click context menus** anywhere (files, tabs, commits, terminal, editor).

### P2: git

12. Staging is per file only: no hunk/line staging, no conflict resolution UI, no gutter change
    markers or blame in the editor, no branch delete/rename, no tag creation, no stash management
    beyond push/pop, no rebase/amend-author/force-push-with-lease, history is capped at 2000 commits
    with no search or branch filter. Diffs are not syntax-highlighted. Push/pull rely on your credential
    helper; an SSH key with a passphrase and no agent will fail (no prompt UI).
13. Git work still runs on the UI thread for **diff rendering** of a huge commit (capped at 1500 rows
    per file, but computing it is synchronous).

### P2: terminal and AI sessions

14. No search in terminal, no clickable URLs/paths, no split sessions, no per-tab working directory
    display, no IME/dead-key input, no bell indicator.
15. **Claude/Copilot are just terminal profiles.** Nothing passes them the current file, selection, or
    diff ("ask about this file", "review this commit"). Worth building: send-selection-to-terminal and
    "open the files the agent just changed".

### P3: performance risks (measured where noted)

16. **One enormous line** (minified JSON/JS): **measured ~47 ms per keystroke on a 3.2 MB line**, because
    the whole line is laid out each frame. *Fix*: lay out only the horizontal slice in view.
17. Find recomputes synchronously on every keystroke: 27 ms at 100k lines but 0.15-0.3 s at 1M lines.
    *Fix*: debounce or run on a background thread for big files.
18. After a far jump, colors are approximate until you scroll through (see above); a background pass
    would make them exact.
19. `Expanded` (tab expansion) is computed twice per visible line per frame; a free ~10 % of frame CPU.
20. Start-up: about 150 ms is GPUI/Metal window creation before mide's code runs (outside our control);
    mide's own part is now about 30 ms.

### P3: distribution and engineering

21. The app is **ad-hoc signed**, not notarized: fine on this Mac, but others get Gatekeeper warnings.
    No auto-update, no crash log or "report a problem", no telemetry (good) but also no diagnostics file.
22. **No CI**, and this repository has never been committed. UI code has no automated tests (GPUI's
    test harness is not wired up); the engine crates do.
23. **GPUI is pre-1.0** (0.2.x) and its API shifts between releases; pin the version and budget time
    for upgrades.
24. Accessibility (VoiceOver) is whatever GPUI provides, which is little.

## Suggested next steps

1. Reload-on-external-change + session restore + Save As / new file (P0, about a week of work).
2. Virtualize the sidebar, add scrollbars, indentation detection (the visible daily annoyances).
3. Find in files.
4. Long-line fix and background find.
5. Then git depth (hunk staging, gutter markers) and agent integration.
