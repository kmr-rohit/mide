# Tasks

Working list for mide. Update it as things move. `[x]` done, `[~]` in progress or waiting on someone, `[ ]` not started.
Owner: **me** = the maintainer (needs accounts, a Mac, or a decision), **claude** = can be done in a Claude Code session.

Last updated: 2026-11-24

## Now

- [~] **Notarize the installer** (owner: me). Needs a paid Apple Developer account: Developer ID Application and Installer
      certificates, an app-specific password, then the repository secrets listed in `docs/notes/RELEASING.md`.
      Once the secrets exist, the Release workflow signs and notarizes the `.pkg` and `.dmg` by itself.
- [ ] **Confirm CI is green** on `3e4d9d3` (owner: me). It was queued for a macOS runner; local clippy, tests, license and site checks pass.
- [ ] **Commit and push the current batch** (owner: me): DMG script, cask script, release workflow change, README comparison,
      roadmap rewrite, `TASKS.md`. Nothing here is committed yet.

## Reach (from the editor comparison)

Distribution
- [x] One-line installer `scripts/install.sh` (no sudo with a DMG release; falls back to the package for older ones; checksum-verified; `--uninstall`). Tested against the real 0.2.0 release with `sudo` stubbed. Docs updated. Becomes the sudo-free path once a release ships a DMG.
- [x] Disk image script `scripts/dmg.sh`, built and mounted locally (unsigned). Added to the release workflow.
- [x] Homebrew cask generator `scripts/cask.sh`, output syntax-checked. Added to the release workflow.
- [ ] Create the `homebrew-mide` tap repo and publish `Casks/mide.rb` (owner: me; steps in `docs/notes/RELEASING.md`).
      Test with `brew install --cask` on a clean Mac after the first release that includes `mide.rb`.
- [ ] Verify the release workflow end to end by tagging the next version (owner: me). The DMG and cask steps in
      `.github/workflows/release.yml` have not run in CI yet.

Show it
- [x] Screenshots of the editor, language servers, git, terminal and palette are on the site and in the README (taken from mide on its own source; the git one has the committer email painted out).
- [x] The homepage already has a demo video (`site-src/assets/mide-demo.mp4`, a Go project, git history and Copilot in the terminal). The README does not show it yet.
- [~] A newer demo recording (owner: me). `scripts/demo.sh record` prepares the 1M-line file and plays a scripted run while `screencapture` records the screen (needs Accessibility and Screen Recording permission; it only types while mide is frontmost). **Not run yet**: rehearse with `scripts/demo.sh play`, then record, then convert with ffmpeg (commands in the script header). Add the GIF to the README and site.
- [ ] Head-to-head benchmark page: the same 1M-line file in mide, Zed, VS Code and Sublime, with the method and script published and
      the cases mide loses included (owner: claude for the script and page, me for running it on the other editors).
- [x] README comparison table, honest about the gaps (LSP, extensions, macOS only).
- [x] Roadmap page brought up to date with what 0.2.0 shipped.

Announce
- [ ] Blog post: "Find on a 35 MB file: 269 ms to under 1 ms" (the block-scan design). Basis for Show HN, r/rust, This Week in Rust, Lobsters.
- [ ] Show HN, after notarization and the demo exist (owner: me).
- [~] Build-in-public posts on X. Reply to the Find question drafted; post it after CI and the site deploy are done.

## Product

- [x] Find: block scan across cores, background search over 4M chars, select-all-occurrences (3e4d9d3).
- [x] Find bar: selection, copy/cut/select-all, Replace toggle chevron.
- [~] **Language server support.**
  - [x] `mide-lsp` crate: JSON-RPC framing, UTF-16 positions, client (initialize, document sync, diagnostics, definition, completion, hover), server lookup. 17 tests with a mock server.
  - [x] Editor: wavy underlines, status bar counts and message under the caret, F12 go to definition, F8 / Shift-F8 problems, `language_servers` setting.
  - [x] Tried for real: the ignored real-server test passes, and in the app a syntax error shows the underline, the status bar count and the message (screenshot on the site).
  - [ ] **Type errors do not show live in this repo.** rust-analyzer returned no diagnostics for a type mismatch in unsaved text (checked with the client outside the GUI, also after a follow-up edit). They are expected to come from `cargo check` after a save, which I have not verified: save a file with a type error, wait for the first `cargo check`, and see if the underline appears. Docs say "after you save" for Rust.
  - [ ] rust-analyzer sends hundreds of progress messages while loading; each one repaints the status bar. Throttle them.
  - [ ] Completion popup (the client already parses completion items; needs the popup UI, key handling and accepting an item).
  - [ ] Hover documentation, go to symbol, find references, rename.
  - [ ] Custom server commands in `config.toml` (needs to respect workspace trust).
  - [ ] Incremental document sync (today the whole file is re-sent after a 250 ms pause; files over about 3 MB are skipped).
  - [ ] Problems list panel.
- [ ] Linux port (GPUI supports it; PTY, file watching and packaging need work).
- [ ] Split panes, soft wrap, code folding, minimap.
- [ ] Send the current diff to an agent session; open the files an agent just changed.
- [ ] Terminal: search, clickable links and paths, split sessions.
- [ ] Git: line staging, conflict resolution, rebase, branch delete/rename, history search.
- [ ] Automatic updates.
- [ ] Faster path for one very long line (3 MB minified line: about 47 ms per keystroke).

## Housekeeping

- [~] Release 0.3.0 (unsigned): version bumped and tagged; check the DMG, the cask and the install script against the real release page. A signed and notarized release follows once the Apple secrets exist.
- [ ] Run the Find bar changes through a manual pass on a real file with the bar open and a big file open
      (done once for the happy path; edit-while-searching on a 1M-line file not yet tried).
- [ ] Add the new "match at the end" benchmarks to `docs/notes/PERFORMANCE.md` if it lists benchmarks by name.
- [ ] The `release.yml` Publish step now uploads `dist/*.dmg` and `dist/*.rb`; check the release page after the next tag.
