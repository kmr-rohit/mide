<p align="center">
  <img src="assets/macos/mide-1024.png" width="128" alt="mide icon">
</p>

<h1 align="center">mide</h1>

<p align="center">
  A native code editor for macOS, written in Rust.<br>
  Sublime-style speed, with git history and terminal sessions in the same window.
</p>

<p align="center">
  <a href="https://github.com/kmr-rohit/mide/releases/latest"><b>Download</b></a> &nbsp;|&nbsp;
  <a href="https://kmr-rohit.github.io/mide/">Documentation</a> &nbsp;|&nbsp;
  <a href="https://kmr-rohit.github.io/mide/performance.html">Benchmarks</a>
</p>

---

## What it is

* **An editor that stays out of the way.** A rope-backed buffer, multi-cursor editing, regex find and replace, the common line commands, and Sublime-style shortcuts. Syntax colors come from Sublime syntax definitions.
* **Git without leaving the window.** A commit graph, side-by-side diffs, and staging, commit, push, pull, branches and stash, using your own `git`, credentials and hooks.
* **Terminals that dock anywhere.** Real sessions for your shell, Claude Code and GitHub Copilot, in a panel you can drag to the bottom, left or right.
* **Native.** Written in Rust and drawn on the GPU with [GPUI](https://www.gpui.rs/). No web view, no Electron. Light and dark themes, saved layout, a command palette, and go-to-file.

## Fast, and measured

| | |
|---|---|
| Work per keystroke | 4.6 ms (frame budget 16 ms) |
| Open a 1,000,000-line file | 63 ms |
| Jump to line 100,000 with colors | 12 ms |
| CPU when idle | under 1% |

Measured on one Apple Silicon Mac with a release build. Every number is reproducible with `scripts/perf.sh`; the full results and methodology are on the [performance page](https://kmr-rohit.github.io/mide/performance.html).

## Install

Download the `.pkg` from the [latest release](https://github.com/kmr-rohit/mide/releases/latest), open it, then:

```sh
mide ~/code/my-project
```

The first time, macOS may warn that the installer is from an unidentified developer (it is not yet notarized); the [install guide](https://kmr-rohit.github.io/mide/install.html) shows how to continue.

## Build from source

You need Rust and the full Xcode app (the GPU interface compiles Metal shaders).

```sh
xcodebuild -downloadComponent MetalToolchain   # once, after installing Xcode
cargo run --release -p mide -- .

scripts/bundle.sh --install    # build mide.app and copy it to /Applications
scripts/pkg.sh                 # build the installer into dist/
scripts/perf.sh --quick        # benchmarks (about a minute)
cargo test --workspace         # about 150 tests
```

If `cargo build` cannot find the `metal` compiler, point it at full Xcode with
`sudo xcode-select -s /Applications/Xcode.app/Contents/Developer` (or set `DEVELOPER_DIR`).

## Repository map

| Path | What is in it |
|---|---|
| `crates/core` | text buffer, undo, find and replace, line commands |
| `crates/syntax` | syntax highlighting and the editor theme |
| `crates/files` | sidebar tree, file index, file watcher |
| `crates/git` | git reading (libgit2) and git operations |
| `crates/fuzzy` | fuzzy matching for go-to-file and the palette |
| `crates/term` | terminal emulation on a PTY |
| `crates/app` | the interface (the `mide` binary) |
| `crates/bench` | benchmarks with budgets |
| `site-src/`, `docs/` | the website source and its generated pages (GitHub Pages serves `docs/`) |
| `scripts/` | bundle, installer, performance and license scripts |

## Publishing and releasing

See [docs/notes/RELEASING.md](docs/notes/RELEASING.md). The website is plain HTML built by `python3 site-src/build.py`.

## License

MIT, see [LICENSE](LICENSE). Third-party licenses: [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md). mide is an independent project, not affiliated with Sublime HQ, Zed, GitHub or Anthropic.
