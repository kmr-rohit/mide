# Changelog

## 0.2.0 (2026-11-23)

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

Windows, menus and git in the editor:

* New Window (⇧⌘N) and Open Folder in New Window (⌥⌘O); the sidebar menu on a folder has "Open in New Window". Quit counts unsaved files in every window.
* Right-click menus: in the editor (cut/copy/paste, find, line commands, comment, send to terminal, stage/revert hunk), on tabs (close, close others, close to the right, copy path, reveal) and in the sidebar.
* Send Selection to Terminal (with `file:lines` and a fenced block) and Send File Path (as `@file` for Claude and Copilot sessions), pasted so nothing runs until you press Enter.
* Tab bar scrolls sideways when full, keeps the active tab in view, truncates long names, and tabs can be dragged to reorder.
* Keybindings file `~/.config/mide/keymap.toml` (Preferences: Edit Keybindings / Reload Keymap; saving it reloads). Chords and unbinding are supported.
* Git diff view wraps long lines onto extra rows (toolbar toggle "Wrap: on/off"), following the window and column widths.
* Git gutter markers (green added, blue modified, red deleted lines versus the staged file), Stage Hunk, Revert Hunk, Next/Previous Change (⌥⌘↓/↑), and the blame of the caret line in the status bar.

Fixed:

* Right-click menu entries did nothing: the press that clicked an entry also dismissed the menu first.
* Closing the last window left a Dock icon that could not bring the app back; the Dock icon, `mide <path>` and Finder now open a window again.

## 0.1.0

First public release.

* Editor: rope buffer, multi-cursor, regex find and replace, line commands, grouped undo, Sublime-style syntax highlighting.
* Git window: commit graph, side-by-side diffs, stage, commit, amend, push, pull, fetch, branch, stash, cherry-pick, revert.
* Terminal panel: sessions, profiles (Shell, Claude Code, GitHub Copilot), docking, mouse support, scrollback.
* Light and dark themes, command palette, go to file, workspace trust for project commands.
* macOS installer package and `mide` command-line launcher.
