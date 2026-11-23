# Changelog

## Unreleased

Data safety (the P0 items from the product review):

* Files changed on disk (by git, a formatter, an AI agent) are noticed. Clean tabs reload by themselves; a tab with unsaved edits shows a bar to **Reload** or **Keep mine**, and saving is blocked until you choose.
* Session restore: reopening a folder brings back its tabs, caret positions and scroll offsets (`[workspace] restore_session`).
* Crash recovery: unsaved text is backed up every few seconds and offered back after a crash.
* New File (⌘N), Save As (⇧⌘S), and a sidebar right-click menu: new file/folder, rename, reveal in Finder, copy path, move to Trash.
* Binary files are refused before they are read into memory.

Everyday editing:

* Find in Files (⇧⌘F): streaming background search honoring `.gitignore`, with case, whole-word and regex toggles.
* The file's indentation (tabs, 2 or 4 spaces) is detected and used for Tab, indent and Enter; the status bar shows it.
* Auto-indent after `{`, `[`, `(` (and `:` in Python and YAML), `{|}` opens into three lines.
* Brackets and quotes close themselves, wrap a selection, step over the closer, and Backspace removes an empty pair.
* Scrollbars in the editor (draggable) and the sidebar; the sidebar list is virtualized, so huge folders stay smooth.
* Clicking the Dock icon after closing the window opens a window again.

## 0.1.0

First public release.

* Editor: rope buffer, multi-cursor, regex find and replace, line commands, grouped undo, Sublime-style syntax highlighting.
* Git window: commit graph, side-by-side diffs, stage, commit, amend, push, pull, fetch, branch, stash, cherry-pick, revert.
* Terminal panel: sessions, profiles (Shell, Claude Code, GitHub Copilot), docking, mouse support, scrollback.
* Light and dark themes, command palette, go to file, workspace trust for project commands.
* macOS installer package and `mide` command-line launcher.
