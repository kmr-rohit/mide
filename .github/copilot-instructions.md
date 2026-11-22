# Copilot instructions for mide

mide is a native macOS code editor in Rust (GPUI, GPU-drawn, no web view) with an integrated git view and terminal panes. It is a Cargo workspace.

## Build, test, lint

Requires Rust and full Xcode (GPUI compiles Metal shaders; `.cargo/config.toml` points `DEVELOPER_DIR` at `/Applications/Xcode.app`). Once: `xcodebuild -downloadComponent MetalToolchain`.

```sh
cargo run --release -p mide -- .          # run the editor on this folder
cargo test --workspace                    # all tests
cargo test -p mide-core <test_name>       # single test (crate names are mide-*: core, files, fuzzy, git, syntax, term)
cargo clippy --workspace --all-targets -- -D warnings
python3 scripts/licenses.py --check       # dependency license allow-list
python3 site-src/build.py                 # regenerate docs/ website
scripts/perf.sh --quick                   # benchmarks (crates/bench)
scripts/bundle.sh --install               # build mide.app; scripts/pkg.sh builds the installer into dist/
```

CI (`.github/workflows/ci.yml`) runs tests, clippy with `-D warnings`, the license check, and requires `docs/` to be up to date: `docs/` is **generated** from `site-src/` (pages, data, template), so edit `site-src/` and re-run `build.py`, never hand-edit `docs/`. Adding a dependency may require updating the license allow-list in `scripts/licenses.py` and `THIRD_PARTY_LICENSES.md`.

## Architecture

UI-free logic lives in small library crates; `crates/app` (binary `mide`) is the only crate that depends on GPUI and wires them together.

- `core`: rope buffer (`ropey`), multi-cursor editing, line ops, regex search, undo. Tests in `tests.rs` / `lineops_tests.rs`.
- `syntax`: highlighting via syntect/two-face (Sublime syntax definitions) and theme mapping.
- `files`: gitignore-aware file tree/scan (`ignore`) and file watching (`notify`).
- `fuzzy`: fuzzy matching (`nucleo-matcher`) for go-to-file and the command palette.
- `git`: libgit2 (`git2`) wrapper: commit graph, diffs, ops (stage/commit/push/pull/branch/stash...). Network ops use the user's own `git`, credentials and hooks.
- `term`: terminal emulation on `alacritty_terminal`.
- `bench`: benchmark harness (fixtures + report) used by `scripts/perf.sh`; depends on the library crates directly, not on the app.
- `app`: GPUI views and glue — `workspace.rs` (top-level window state), `editor.rs`, `git_view.rs`, `terminal_view.rs`, `palette.rs`, `find_bar.rs`, `layout.rs` (docking), `keys.rs`/`input.rs` (key bindings and input), `theme.rs`, `config.rs` (TOML), `trust.rs` (workspace trust for project commands), `recent.rs`, `perf.rs`.

Entry point `main.rs` handles `mide [path]` (folder = project, file = its folder + that file) and queues open-URL events (Finder, `open -a`, the `mide` CLI launcher in `assets/macos/`) through an async channel since they can arrive before the window exists.

## Conventions

- Keep editor/git/term/files logic out of `crates/app`; put it in the matching library crate with tests in that crate (tests live in `tests.rs` modules, not in a separate `tests/` dir).
- Performance is a product feature (see `DESIGN.md` principles): startup <100 ms, one frame of input latency, huge files open instantly. Avoid per-keystroke work that scales with file size; verify with `scripts/perf.sh`.
- UI follows `DESIGN.md` (Sublime Text/Merge look: light chrome, dark editor, no toasts or modals where a palette will do). Every feature should be reachable from the command palette with its shortcut displayed.
- Clippy must pass with warnings denied.
- Update `CHANGELOG.md` for user-visible changes.
