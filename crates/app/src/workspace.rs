//! The window's root view: sidebar, tab bar, active editor, status bar.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    WeakEntity, actions, div, prelude::*, px, rgb, svg, App, ClickEvent, Context, Entity, EntityId, FocusHandle,
    Focusable, FontWeight, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PathPromptOptions, Render, SharedString, Window, CursorStyle, rgba,
};
use mide_core::Buffer;
use mide_fuzzy::Kind;
use mide_files::{Changes, FileTree, Watcher};
use mide_git::{Repo, StatusSummary};
use mide_syntax::{Syntaxes, Theme};

use crate::assets::file_icon;
use crate::config::{self, Config, Tools};
use crate::editor::EditorView;
use crate::fonts::{self, Fonts};
use crate::git_view::{GitEvent, GitView};
use crate::layout::{self, Dock, Layout};
use crate::perf;
use crate::theme::{self, ui};
use crate::trust;
use crate::palette::{Choice, Command, Item, Palette, PaletteEvent};
use crate::terminal_view::{TerminalEvent, TerminalView};
use crate::recent;

actions!(
    workspace,
    [Save, CloseTab, NextTab, PrevTab, Quit, ToggleGit, OpenFolder, QuickOpen, CommandPalette, ToggleTerminal, NewTerminal, ToggleSidebar, CloseFolder, ToggleTheme, GotoLine]
);

/// Payload for dragging the terminal panel by its header to another dock.
#[derive(Clone)]
struct DraggedPanel;

/// The little label that follows the cursor while dragging a panel.
struct DragGhost(SharedString);

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(12.))
            .py(px(6.))
            .rounded(px(5.))
            .bg(rgb(ui().accent))
            .text_color(rgb(ui().on_accent))
            .text_size(px(12.))
            .shadow_lg()
            .child(self.0.clone())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResizeTarget {
    Sidebar,
    TerminalBottom,
    TerminalLeft,
    TerminalRight,
}

struct ResizeDrag {
    target: ResizeTarget,
    /// Mouse coordinate (x, or y for the bottom dock) when the drag began.
    start: f32,
    /// The panel's size when the drag began.
    size: f32,
}
const MAX_QUICK_OPEN_FILES: usize = 50_000;

struct Session {
    name: String,
    view: Entity<TerminalView>,
}

enum PaletteKind {
    GotoLine,
    ConfirmSwitch { dirty: usize, name: String },
    Files,
    Commands,
    Profiles,
}


const ROW_HEIGHT: f32 = 22.0;

pub struct Workspace {
    root: Option<PathBuf>,
    tree: Option<FileTree>,
    recent: Vec<PathBuf>,
    layout: Layout,
    resizing: Option<ResizeDrag>,
    config: Config,
    /// What the background probe of your login shell found (see `probe_login_shell`).
    probed_tools: Tools,
    terminals: Vec<Session>,
    active_term: usize,
    show_term: bool,
    pending_term_close: Option<EntityId>,
    /// Folder waiting for the "discard unsaved changes?" answer.
    pending_open: Option<PathBuf>,
    palette: Option<Entity<Palette>>,
    palette_prev_focus: Option<FocusHandle>,
    tabs: Vec<Entity<EditorView>>,
    active: usize,
    syntaxes: Arc<Syntaxes>,
    theme: Arc<Theme>,
    focus_handle: FocusHandle,
    git: Option<StatusSummary>,
    git_view: Option<Entity<GitView>>,
    show_git: bool,
    message: Option<SharedString>,
    message_gen: u64,
    pending_discard: Option<EntityId>,
    pending_quit: bool,
    last_title: String,
    fs_rx: Option<Receiver<Changes>>,
    _watcher: Option<Watcher>,
}

impl Workspace {
    pub fn new(root: Option<PathBuf>, initial: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        perf::mark("workspace: new() begins");
        let mut ws = Self {
            root: None,
            tree: None,
            recent: recent::load(),
            layout: Layout::load(),
            resizing: None,
            config: Config::defaults(Tools::default()),
            probed_tools: Tools::default(),
            terminals: Vec::new(),
            active_term: 0,
            show_term: false,
            pending_term_close: None,
            pending_open: None,
            palette: None,
            palette_prev_focus: None,
            tabs: Vec::new(),
            active: 0,
            syntaxes: Syntaxes::load_default(),
            theme: Arc::new(Theme::default_dark()),
            focus_handle: cx.focus_handle(),
            git: None,
            git_view: None,
            show_git: false,
            message: None,
            message_gen: 0,
            pending_discard: None,
            pending_quit: false,
            last_title: String::new(),
            fs_rx: None,
            _watcher: None,
        };
        perf::mark("workspace: struct built (syntaxes, layout, recents)");
        theme::set(ws.layout.theme);
        window.focus(&ws.focus_handle);
        ws.load_config(cx);
        perf::mark("workspace: config + fonts loaded");
        if let Some(root) = root {
            ws.open_folder(root, window, cx);
        }
        if let Some(path) = initial {
            ws.open_file(&path, window, cx);
        }
        if std::env::var_os("MIDE_OPEN_GIT").is_some() {
            ws.set_git_visible(true, window, cx);
        }

        if let Some(n) = std::env::var("MIDE_PERF_TYPING").ok().and_then(|v| v.parse::<usize>().ok()) {
            ws.spawn_perf_script(n, cx);
        }

        // Learn which AI CLIs your login shell can run (Finder-launched apps lack your PATH).
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let tools = cx.background_executor().spawn(async { config::probe_login_shell() }).await;
            let _ = this.update(cx, |ws, cx| {
                if ws.probed_tools != tools {
                    ws.probed_tools = tools;
                    ws.load_config(cx);
                    cx.notify();
                }
            });
        })
        .detach();

        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(200)).await;
            if this.update(cx, |ws, cx| ws.poll_fs(cx)).is_err() {
                break;
            }
        })
        .detach();
        ws
    }

    /// Open `path` as the project folder.
    ///
    /// With a project already open, this replaces the whole workspace: its files and
    /// terminal sessions are closed (set `[workspace] keep_open_on_switch = true` to
    /// keep them instead). If any file has unsaved changes you are asked first.
    pub fn open_folder(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.root.is_none() || self.config.keep_open_on_switch {
            return self.switch_folder(path, false, window, cx);
        }
        let dirty = self.tabs.iter().filter(|t| t.read(cx).buffer.is_dirty()).count();
        if dirty == 0 {
            return self.switch_folder(path, true, window, cx);
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.pending_open = Some(path);
        self.close_palette(false);
        self.open_palette(PaletteKind::ConfirmSwitch { dirty, name }, window, cx);
    }

    /// Make `path` the project: the sidebar, git state, watcher and config follow it.
    /// `close_all` also closes the previous workspace's files and terminals.
    fn switch_folder(&mut self, path: PathBuf, close_all: bool, window: &mut Window, cx: &mut Context<Self>) {
        let tree = match FileTree::open(&path) {
            Ok(t) => t,
            Err(e) => {
                self.flash(format!("Cannot open {}: {e}", path.display()), cx);
                return;
            }
        };
        let root = tree.root().to_owned();
        perf::mark("open_folder: tree read");
        let (watcher, fs_rx) = match Watcher::new(&root) {
            Ok((w, rx)) => (Some(w), Some(rx)),
            Err(e) => {
                eprintln!("file watching disabled: {e}");
                (None, None)
            }
        };
        perf::mark("open_folder: watcher started");
        self.close_palette(false);
        self.pending_quit = false;
        if close_all {
            // Dropping a terminal view ends its session without blocking.
            self.tabs.clear();
            self.active = 0;
            self.pending_discard = None;
            self.terminals.clear();
            self.active_term = 0;
            self.show_term = false;
            self.pending_term_close = None;
        }
        self.git = None;
        self.git_view = None;
        self.show_git = false;
        self.tree = Some(tree);
        self.root = Some(root.clone());
        self.fs_rx = fs_rx;
        self._watcher = watcher;
        recent::push(&mut self.recent, &root, recent::MAX_RECENT);
        recent::save(&self.recent);

        self.load_config(cx);
        // Start this project's configured sessions only if nothing is running yet,
        // so switching folders never piles up duplicate sessions.
        if self.terminals.is_empty() {
            for name in self.config.open_terminals.clone() {
                self.spawn_terminal(&name, window, cx);
            }
        }
        perf::mark("open_folder: config loaded");
        // Git status and history load in the background; ⌃⇧G is ready when they finish.
        if Repo::discover(&root).is_ok() {
            self.ensure_git_view(window, cx);
        }
        perf::mark("open_folder: git view ready");
        self.refresh_git(cx);
        if let Some(path) = self.active_editor().and_then(|e| e.read(cx).path.clone()) {
            if let Some(t) = self.tree.as_mut() {
                let _ = t.reveal(&path);
            }
        }

        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let kept = match (self.tabs.len(), self.terminals.len()) {
            _ if close_all => String::new(),
            (0, 0) => String::new(),
            (f, t) => format!(" — kept {f} open file{} and {t} terminal session{}", plural(f), plural(t)),
        };
        self.flash(format!("Opened {name}{kept}"), cx);
        perf::mark(&format!("folder opened ({name})"));
        if !(self.show_term && !self.terminals.is_empty() && self.terminal_has_focus(window, cx)) {
            self.focus_main(window, cx);
        }
        cx.notify();
    }

    /// Close the project folder but keep open files and terminal sessions.
    fn close_folder(&mut self, _: &CloseFolder, window: &mut Window, cx: &mut Context<Self>) {
        if self.root.is_none() {
            return;
        }
        self.root = None;
        self.tree = None;
        self.fs_rx = None;
        self._watcher = None;
        self.git = None;
        self.git_view = None;
        self.show_git = false;
        self.close_palette(false);
        self.config = Config::defaults(Tools::default());
        self.flash("Folder closed — open files and terminal sessions are still open", cx);
        self.focus_main(window, cx);
        cx.notify();
    }

    fn prompt_open_folder(&mut self, _: &OpenFolder, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(p) = paths.into_iter().next() {
                    let _ = this.update_in(cx, |ws, window, cx| ws.open_folder(p, window, cx));
                }
            }
        })
        .detach();
    }

    /// Re-read branch / clean-or-dirty state on a background thread (a huge repo
    /// can take seconds, and the UI must never wait for that).
    fn refresh_git(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            self.git = None;
            return;
        };
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let for_root = root.clone();
            let summary = cx
                .background_executor()
                .spawn(async move { Repo::discover(&for_root).ok().and_then(|r| r.status_summary().ok()) })
                .await;
            let _ = this.update(cx, |ws, cx| {
                // Ignore the answer if the project changed while we were asking.
                if ws.root.as_ref() == Some(&root) {
                    ws.git = summary;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn poll_fs(&mut self, cx: &mut Context<Self>) {
        let Some(rx) = &self.fs_rx else { return };
        let mut any = false;
        let mut git = false;
        while let Ok(c) = rx.try_recv() {
            if let Some(t) = self.tree.as_mut() {
                t.apply(&c);
            }
            git |= c.git || !c.paths.is_empty();
            any = true;
        }
        if git {
            self.refresh_git(cx);
            if let Some(v) = &self.git_view {
                v.update(cx, |v, cx| v.reload(cx));
            }
        }
        if any {
            cx.notify();
        }
    }

    fn flash(&mut self, msg: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.message = Some(msg.into());
        self.message_gen += 1;
        let generation = self.message_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(4)).await;
            let _ = this.update(cx, |ws, cx| {
                if ws.message_gen == generation {
                    ws.message = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn active_editor(&self) -> Option<&Entity<EditorView>> {
        self.tabs.get(self.active)
    }

    // ---- tabs ----------------------------------------------------------------------

    pub fn open_file(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        if let Some(i) = self.tabs.iter().position(|t| t.read(cx).path.as_deref() == Some(path.as_path())) {
            self.activate(i, window, cx);
            return;
        }
        let buffer = match Buffer::open(&path) {
            Ok(b) => b,
            Err(e) => {
                self.flash(format!("Cannot open {}: {e}", path.display()), cx);
                return;
            }
        };
        if buffer.is_lossy() {
            self.flash("File is not valid UTF-8: opened read-only-ish (saving is disabled)", cx);
        }
        let (syntaxes, theme) = (self.syntaxes.clone(), self.theme.clone());
        let p = path.clone();
        let editor = cx.new(|cx| EditorView::new(buffer, Some(p), syntaxes, theme, cx));
        cx.observe(&editor, |_, _, cx| cx.notify()).detach();
        self.tabs.push(editor);
        if let Some(t) = self.tree.as_mut() {
            let _ = t.reveal(&path);
        }
        self.activate(self.tabs.len() - 1, window, cx);
    }

    fn activate(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if i >= self.tabs.len() {
            return;
        }
        self.active = i;
        self.pending_discard = None;
        let handle = self.tabs[i].read(cx).focus_handle(cx);
        window.focus(&handle);
        cx.notify();
    }

    fn close_active(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_has_focus(window, cx) {
            self.close_session(self.active_term, false, window, cx);
        } else {
            self.close_tab(self.active, window, cx);
        }
    }

    /// Close tab `i`. A tab with unsaved changes needs a second request.
    fn close_tab(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.tabs.get(i).cloned() else { return };
        let id = editor.entity_id();
        if editor.read(cx).buffer.is_dirty() && self.pending_discard != Some(id) {
            self.pending_discard = Some(id);
            self.flash("Unsaved changes: click × or press ⌘W again to discard them", cx);
            return;
        }
        self.pending_discard = None;
        self.tabs.remove(i);
        if i < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1).min(self.tabs.len().saturating_sub(1));
        }
        if self.tabs.is_empty() {
            window.focus(&self.focus_handle);
        } else {
            self.activate(self.active, window, cx);
        }
        cx.notify();
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.is_empty() {
            self.activate((self.active + 1) % self.tabs.len(), window, cx);
        }
    }

    fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.is_empty() {
            self.activate((self.active + self.tabs.len() - 1) % self.tabs.len(), window, cx);
        }
    }

    fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor().cloned() else { return };
        let result = editor.update(cx, |ed, cx| ed.save(cx).map(|()| ed.title()));
        let saved_path = editor.read(cx).path.clone();
        match result {
            Ok(name) => {
                self.flash(format!("Saved {name}"), cx);
                let is_config = saved_path.is_some_and(|p| {
                    Some(&p) == config::global_path().as_ref()
                        || self.root.as_ref().is_some_and(|r| p == config::project_path(r))
                });
                if is_config {
                    self.load_config(cx);
                    if self.message.as_deref().is_none_or(|m| !m.starts_with("Config error")) {
                        self.flash("Saved. Config reloaded", cx);
                    }
                }
            }
            Err(e) => self.flash(e, cx),
        }
        self.refresh_git(cx);
    }

    fn toggle_git(&mut self, _: &ToggleGit, window: &mut Window, cx: &mut Context<Self>) {
        self.set_git_visible(!self.show_git, window, cx);
    }

    /// Create (or refresh) the git view for the current folder.
    fn ensure_git_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<GitView>> {
        let root = self.root.clone()?;
        if let Some(v) = &self.git_view {
            v.update(cx, |v, cx| v.reload(cx));
            return Some(v.clone());
        }
        let (syntaxes, theme, widths) = (self.syntaxes.clone(), self.theme.clone(), (self.layout.git_locations_width, self.layout.git_commits_width));
        let v = cx.new(|cx| GitView::new(root, syntaxes, theme, widths, cx));
        cx.subscribe_in(&v, window, |ws, _, ev: &GitEvent, window, cx| match ev {
            GitEvent::Close => ws.set_git_visible(false, window, cx),
            GitEvent::Refreshed => {
                ws.refresh_git(cx);
                cx.notify();
            }
            GitEvent::LayoutChanged(locations, commits) => {
                ws.layout.git_locations_width = *locations;
                ws.layout.git_commits_width = *commits;
                ws.layout = ws.layout.clone().sanitized();
                ws.layout.save();
            }
            GitEvent::OpenFile(rel) => {
                let path = ws.root.clone().unwrap_or_default().join(rel);
                ws.set_git_visible(false, window, cx);
                ws.open_file(&path, window, cx);
            }
        })
        .detach();
        self.git_view = Some(v.clone());
        Some(v)
    }

    fn set_git_visible(&mut self, show: bool, window: &mut Window, cx: &mut Context<Self>) {
        if show {
            let Some(view) = self.ensure_git_view(window, cx) else {
                self.flash("Open a folder first (⌘O)", cx);
                return;
            };
            self.show_git = true;
            window.focus(&view.read(cx).focus_handle(cx));
        } else {
            self.show_git = false;
            if let Some(ed) = self.active_editor() {
                let handle = ed.read(cx).focus_handle(cx);
                window.focus(&handle);
            } else {
                window.focus(&self.focus_handle);
            }
        }
        cx.notify();
    }

    /// `MIDE_PERF_TYPING=<n>`: drive `n` keystrokes, then scrolling, then a jump to the
    /// end of the file in the active editor, and print latency statistics. Set
    /// `MIDE_PERF_EXIT=1` to quit afterwards. See `scripts/perf.sh`.
    fn spawn_perf_script(&mut self, n: usize, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let exec = cx.background_executor().clone();
            let wait = move |ms: u64| exec.timer(Duration::from_millis(ms));
            wait(1500).await; // let the first paint and syntax loading settle
            perf::set_quiet(true);
            let editor = |cx: &mut gpui::AsyncApp| this.update(cx, |ws, _| ws.active_editor().cloned()).ok().flatten();
            let Some(ed) = editor(cx) else {
                eprintln!("[perf-summary] no editor open: pass a file to mide");
                return;
            };

            perf::clear();
            for i in 0..n {
                let _ = ed.update(cx, |ed, cx| ed.perf_type(i, cx));
                wait(25).await;
            }
            perf::summary("typing");

            perf::clear();
            for _ in 0..200 {
                let _ = ed.update(cx, |ed, cx| ed.perf_scroll(3.0, cx));
                wait(16).await;
            }
            perf::summary("scrolling (3 lines per frame)");

            perf::clear();
            let _ = ed.update(cx, |ed, cx| {
                let last = ed.buffer.len_lines();
                ed.goto(last, 1, cx);
            });
            wait(400).await;
            perf::summary("jump to end of file");

            perf::write_json();
            if std::env::var_os("MIDE_PERF_EXIT").is_some() {
                let _ = cx.update(|cx| cx.quit());
            }
        })
        .detach();
    }

    /// Open file:// URLs / paths handed to us by macOS (Finder, `open -a`, Dock drops).
    pub fn open_urls(&mut self, urls: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        for url in urls {
            if let Some(path) = url_to_path(&url) {
                self.open_path(&path, window, cx);
            }
        }
    }

    /// A folder becomes the project; a file opens in the current project, or
    /// brings its folder along when nothing is open yet.
    pub fn open_path(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if path.is_dir() {
            self.open_folder(path.to_owned(), window, cx);
        } else if path.is_file() {
            if self.root.is_none() {
                if let Some(dir) = path.parent() {
                    self.open_folder(dir.to_owned(), window, cx);
                }
            }
            self.open_file(path, window, cx);
        }
        cx.activate(true);
    }

    // ---- config ------------------------------------------------------------------------

    fn load_config(&mut self, cx: &mut Context<Self>) {
        let trusted = self.root.as_deref().is_some_and(trust::is_trusted);
        let loaded = config::load(self.root.as_deref(), self.probed_tools, trusted);
        self.config = loaded.config;
        if let Some(e) = loaded.errors.first() {
            self.flash(format!("Config error (using defaults for that file): {e}"), cx);
        }
        self.apply_fonts(cx);
        if loaded.blocked_commands {
            self.flash(
                "This folder's .mide/config.toml defines terminal commands. They are NOT run until you \
                 trust the folder: command palette ▸ “Workspace: Trust This Folder”",
                cx,
            );
        }
    }

    /// Allow this folder's own `.mide/config.toml` to define terminal commands.
    fn trust_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        if let Err(e) = trust::trust(&root) {
            self.flash(format!("Could not save trust: {e}"), cx);
            return;
        }
        self.load_config(cx);
        if self.terminals.is_empty() {
            for name in self.config.open_terminals.clone() {
                self.spawn_terminal(&name, window, cx);
            }
        }
        self.flash("Folder trusted: its terminal profiles and sessions are enabled", cx);
    }

    /// Resolve the configured fonts against what is installed and apply them.
    fn apply_fonts(&mut self, cx: &mut Context<Self>) {
        // Probe one family at a time instead of listing every installed font (~70 ms):
        // a family that is missing resolves to the same fallback as a nonsense name.
        let ts = cx.text_system().clone();
        let font = |name: &str| gpui::Font {
            family: name.to_owned().into(),
            features: gpui::FontFeatures::default(),
            fallbacks: None,
            weight: gpui::FontWeight::NORMAL,
            style: gpui::FontStyle::Normal,
        };
        let nothing = ts.resolve_font(&font("\u{1}no-such-font"));
        let exists = |name: &str| ts.resolve_font(&font(name)) != nothing;

        let editor = fonts::resolve_with(&self.config.editor_font_family, exists);
        let term = fonts::resolve_with(self.config.terminal_family(), exists);
        let missing = editor != self.config.editor_font_family;
        let wanted = self.config.editor_font_family.clone();
        let new = Fonts {
            editor_family: editor.clone().into(),
            editor_size: self.config.editor_font_size,
            term_family: term.into(),
            term_size: self.config.terminal_size(),
        };
        let changed = fonts::get() != new;
        fonts::set(new);
        if missing {
            self.flash(format!("Font “{wanted}” is not installed; using {editor}"), cx);
        }
        if changed {
            cx.refresh_windows();
        }
    }

    fn edit_config(&mut self, project: bool, window: &mut Window, cx: &mut Context<Self>) {
        let path = if project {
            let Some(root) = &self.root else {
                self.flash("Open a folder first (⌘O)", cx);
                return;
            };
            config::project_path(root)
        } else {
            let Some(p) = config::global_path() else { return };
            p
        };
        if !path.exists() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(&path, config::TEMPLATE) {
                self.flash(format!("Cannot create {}: {e}", path.display()), cx);
                return;
            }
        }
        self.open_file(&path, window, cx);
    }

    // ---- terminals ---------------------------------------------------------------------

    fn terminal_has_focus(&self, window: &Window, cx: &App) -> bool {
        self.show_term
            && self.terminals.get(self.active_term).is_some_and(|s| s.view.read(cx).focus_handle(cx).is_focused(window))
    }

    fn spawn_terminal(&mut self, profile_name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            self.flash("Open a folder first (⌘O)", cx);
            return;
        };
        let profile = self.config.profile(profile_name).cloned().unwrap_or_else(|| config::Profile {
            name: profile_name.to_owned(),
            ..Default::default()
        });
        let opts = config::spawn_options(&profile, &root, self.config.scrollback);
        let view = match TerminalView::spawn(opts, cx) {
            Ok(v) => v,
            Err(e) => {
                self.flash(format!("Cannot start '{profile_name}': {e}"), cx);
                return;
            }
        };
        // "Claude", "Claude 2", "Claude 3", ...
        let taken = self.terminals.iter().filter(|s| s.name == profile.name || s.name.starts_with(&format!("{} ", profile.name))).count();
        let name = if taken == 0 { profile.name.clone() } else { format!("{} {}", profile.name, taken + 1) };

        cx.subscribe_in(&view, window, |ws, view, ev: &TerminalEvent, window, cx| match ev {
            TerminalEvent::TitleChanged => cx.notify(),
            TerminalEvent::Exited(code) => {
                let Some(ix) = ws.terminals.iter().position(|s| &s.view == view) else { return };
                if code.is_none_or(|c| c == 0) {
                    ws.close_session(ix, true, window, cx);
                } else {
                    let name = ws.terminals[ix].name.clone();
                    ws.flash(format!("'{name}' exited with code {}. ⌘W closes the tab", code.unwrap_or(-1)), cx);
                }
            }
        })
        .detach();
        let handle = view.read(cx).focus_handle(cx);
        self.terminals.push(Session { name, view });
        self.active_term = self.terminals.len() - 1;
        self.show_term = true;
        self.show_git = false;
        window.focus(&handle);
        cx.notify();
    }

    /// Close a session. A still-running one needs a second request unless `force`.
    fn close_session(&mut self, ix: usize, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.terminals.get(ix) else { return };
        let id = session.view.entity_id();
        let running = session.view.read(cx).exited.is_none();
        if running && !force && self.pending_term_close != Some(id) {
            self.pending_term_close = Some(id);
            let name = session.name.clone();
            self.flash(format!("'{name}' is still running: press ⌘W (or click ×) again to end it"), cx);
            return;
        }
        self.pending_term_close = None;
        self.terminals.remove(ix); // dropping the view ends the process
        self.active_term = self.active_term.min(self.terminals.len().saturating_sub(1));
        if self.terminals.is_empty() {
            self.show_term = false;
            self.focus_main(window, cx);
        } else {
            let h = self.terminals[self.active_term].view.read(cx).focus_handle(cx);
            window.focus(&h);
        }
        cx.notify();
    }

    fn focus_main(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_editor() {
            Some(ed) => {
                let h = ed.read(cx).focus_handle(cx);
                window.focus(&h);
            }
            None => window.focus(&self.focus_handle),
        }
    }

    fn toggle_terminal(&mut self, _: &ToggleTerminal, window: &mut Window, cx: &mut Context<Self>) {
        if self.root.is_none() && self.terminals.is_empty() {
            self.flash("Open a folder first (⌘O)", cx);
            return;
        }
        if self.show_term {
            self.show_term = false;
            self.focus_main(window, cx);
        } else if self.terminals.is_empty() {
            self.spawn_terminal("Shell", window, cx);
        } else {
            self.show_term = true;
            self.show_git = false;
            let h = self.terminals[self.active_term].view.read(cx).focus_handle(cx);
            window.focus(&h);
        }
        cx.notify();
    }

    fn new_terminal(&mut self, _: &NewTerminal, window: &mut Window, cx: &mut Context<Self>) {
        match self.config.profiles.as_slice() {
            [only] => {
                let name = only.name.clone();
                self.spawn_terminal(&name, window, cx)
            }
            _ => self.open_palette(PaletteKind::Profiles, window, cx),
        }
    }

    // ---- panels: resize, dock, drag & drop -----------------------------------------------

    fn begin_resize(&mut self, target: ResizeTarget, e: &MouseDownEvent) {
        let (start, size) = match target {
            ResizeTarget::Sidebar => (f32::from(e.position.x), self.layout.sidebar_width),
            ResizeTarget::TerminalBottom => (f32::from(e.position.y), self.layout.terminal_height),
            ResizeTarget::TerminalLeft | ResizeTarget::TerminalRight => {
                (f32::from(e.position.x), self.layout.terminal_width)
            }
        };
        self.resizing = Some(ResizeDrag { target, start, size });
    }

    fn on_root_mouse_move(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = &self.resizing else { return };
        if e.pressed_button != Some(MouseButton::Left) {
            // The button was released somewhere we didn't see (e.g. outside the window).
            self.end_resize(cx);
            return;
        }
        let win = window.bounds().size;
        let (x, y) = (f32::from(e.position.x), f32::from(e.position.y));
        match d.target {
            ResizeTarget::Sidebar => {
                self.layout.sidebar_width = d.size + x - d.start;
            }
            ResizeTarget::TerminalBottom => {
                // Dragging the handle up makes the panel taller.
                let max = (f32::from(win.height) - 140.0).max(layout::TERMINAL_HEIGHT_RANGE.0);
                self.layout.terminal_height = (d.size - (y - d.start)).min(max);
            }
            ResizeTarget::TerminalLeft => {
                let max = (f32::from(win.width) - 360.0).max(layout::TERMINAL_WIDTH_RANGE.0);
                self.layout.terminal_width = (d.size + x - d.start).min(max);
            }
            ResizeTarget::TerminalRight => {
                let max = (f32::from(win.width) - 360.0).max(layout::TERMINAL_WIDTH_RANGE.0);
                self.layout.terminal_width = (d.size - (x - d.start)).min(max);
            }
        }
        self.layout = self.layout.clone().sanitized();
        cx.notify();
    }

    fn end_resize(&mut self, cx: &mut Context<Self>) {
        if self.resizing.take().is_some() {
            self.layout.save();
            cx.notify();
        }
    }

    fn on_root_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.end_resize(cx);
    }

    fn resize_handle(&self, id: &'static str, target: ResizeTarget, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let bar = target == ResizeTarget::TerminalBottom;
        let dark = target != ResizeTarget::Sidebar;
        div()
            .id(id)
            .flex_none()
            .when(bar, |d| d.h(px(5.)).w_full().cursor(CursorStyle::ResizeUpDown))
            .when(!bar, |d| d.w(px(5.)).h_full().cursor(CursorStyle::ResizeLeftRight))
            .bg(rgb(if dark { ui().pane_border } else { ui().handle }))
            .hover(|h| h.bg(rgb(ui().accent)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _, _| this.begin_resize(target, e)),
            )
    }

    fn dock_terminal(&mut self, dock: Dock, window: &mut Window, cx: &mut Context<Self>) {
        self.layout.terminal_dock = dock;
        self.layout.save();
        if let Some(s) = self.terminals.get(self.active_term) {
            let h = s.view.read(cx).focus_handle(cx);
            window.focus(&h);
        }
        self.flash(format!("Terminal docked {}", dock.label()), cx);
    }

    /// Switch the app (and terminal) theme. The editor's own theme is unaffected.
    fn set_theme(&mut self, mode: theme::Mode, cx: &mut Context<Self>) {
        theme::set(mode);
        self.layout.theme = mode;
        self.layout.save();
        cx.refresh_windows();
        self.flash(format!("{} theme", mode.label()), cx);
    }

    fn toggle_theme(&mut self, _: &ToggleTheme, _: &mut Window, cx: &mut Context<Self>) {
        self.set_theme(theme::mode().toggled(), cx);
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.layout.show_sidebar = !self.layout.show_sidebar;
        self.layout.save();
        cx.notify();
    }

    /// Translucent targets that appear over the window while a panel is dragged.
    fn render_drop_zones(&self, cx: &mut Context<Self>) -> gpui::Div {
        let zone = |id: &'static str, label: &'static str, dock: Dock, cx: &mut Context<Self>| {
            div()
                .id(id)
                .absolute()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(13.))
                .text_color(rgb(ui().on_accent))
                .bg(rgba((ui().accent << 8) | 0x38))
                .border_2()
                .border_color(rgba((ui().accent << 8) | 0x99))
                .rounded(px(6.))
                .drag_over::<DraggedPanel>(|s, _, _, _| s.bg(rgba((ui().accent << 8) | 0xa0)).border_color(rgb(ui().accent)))
                .on_drop(cx.listener(move |this, _: &DraggedPanel, window, cx| this.dock_terminal(dock, window, cx)))
                .child(label)
        };
        // Leave the status bar (22px) uncovered.
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom(px(22.))
            .child(zone("zone-left", "Dock left", Dock::Left, cx).top(px(6.)).bottom(px(6.)).left(px(6.)).w_1_4())
            .child(zone("zone-right", "Dock right", Dock::Right, cx).top(px(6.)).bottom(px(6.)).right(px(6.)).w_1_4())
            .child(zone("zone-bottom", "Dock bottom", Dock::Bottom, cx).left_1_4().right_1_4().bottom(px(6.)).h_1_3())
    }

    // ---- palette -----------------------------------------------------------------------

    fn open_palette(&mut self, kind: PaletteKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_palette(true);
            self.refocus_after_palette(window, cx);
            return;
        }
        let goto = matches!(kind, PaletteKind::GotoLine);
        let (items, text_kind, placeholder) = match kind {
            PaletteKind::GotoLine => (Vec::new(), Kind::Text, "Go to line (e.g. 120 or 120:8)"),
            PaletteKind::ConfirmSwitch { dirty, name } => {
                let items = vec![
                    Item {
                        label: format!("Discard unsaved changes in {dirty} file{} and open {name}", plural(dirty)),
                        shortcut: "↵".into(),
                        choice: Choice::Index(0),
                    },
                    Item { label: "Cancel".into(), shortcut: "esc".into(), choice: Choice::Index(1) },
                ];
                (items, Kind::Text, "You have unsaved changes")
            }
            PaletteKind::Files => {
                let Some(root) = self.root.clone() else {
                    self.flash("Open a folder first (⌘O)", cx);
                    return;
                };
                let items = mide_files::list_files(&root, MAX_QUICK_OPEN_FILES)
                    .into_iter()
                    .map(|rel| Item { shortcut: String::new(), choice: Choice::File(root.join(&rel)), label: rel })
                    .collect();
                (items, Kind::Path, "Go to file…")
            }
            PaletteKind::Commands => (self.command_items(), Kind::Text, "Type a command…"),
            PaletteKind::Profiles => {
                let items = self
                    .config
                    .profiles
                    .iter()
                    .map(|p| Item {
                        label: match &p.command {
                            Some(c) => format!("{} — {c}", p.name),
                            None => p.name.clone(),
                        },
                        shortcut: String::new(),
                        choice: Choice::Terminal(p.name.clone()),
                    })
                    .collect();
                (items, Kind::Text, "New terminal session…")
            }
        };
        let palette = if goto {
            cx.new(|cx| Palette::input(placeholder, cx))
        } else {
            cx.new(|cx| Palette::new(items, text_kind, placeholder, cx))
        };
        cx.subscribe_in(&palette, window, |ws, _, ev: &PaletteEvent, window, cx| match ev {
            PaletteEvent::Dismissed => {
                ws.pending_open = None;
                ws.close_palette(true);
                ws.refocus_after_palette(window, cx);
                cx.notify();
            }
            PaletteEvent::Chosen(choice) => {
                ws.close_palette(false);
                ws.run_choice(choice.clone(), window, cx);
                cx.notify();
            }
        })
        .detach();
        self.palette_prev_focus = window.focused(cx);
        window.focus(&palette.read(cx).focus_handle(cx));
        self.palette = Some(palette);
        cx.notify();
    }

    fn close_palette(&mut self, _restore: bool) {
        self.palette = None;
    }

    fn refocus_after_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.palette_prev_focus.take() {
            Some(h) => window.focus(&h),
            None => self.focus_main(window, cx),
        }
    }

    fn command_items(&self) -> Vec<Item> {
        let cmd = |label: &str, shortcut: &str, c: Command| Item {
            label: label.to_owned(),
            shortcut: shortcut.to_owned(),
            choice: Choice::Command(c),
        };
        let mut items = vec![
            cmd("File: Open Folder…", "⌘O", Command::OpenFolder),
            cmd("File: Close Folder", "", Command::CloseFolder),
            cmd("File: Go to File…", "⌘P", Command::QuickOpen),
            cmd("Edit: Go to Line…", "⌃G", Command::GotoLine),
            cmd("File: Save", "⌘S", Command::Save),
            cmd("File: Close Tab", "⌘W", Command::CloseTab),
            cmd("View: Next Tab", "⌃Tab", Command::NextTab),
            cmd("View: Previous Tab", "⌃⇧Tab", Command::PrevTab),
            cmd("Git: Show History", "⌃⇧G", Command::ToggleGit),
            cmd("Terminal: Toggle Panel", "⌃`", Command::ToggleTerminal),
            cmd("Terminal: New Session…", "⌘⇧T", Command::NewTerminal),
            cmd("Terminal: Close Session", "⌘W", Command::CloseTerminal),
            cmd("Terminal: Dock Bottom", "", Command::DockBottom),
            cmd("Terminal: Dock Left", "", Command::DockLeft),
            cmd("Terminal: Dock Right", "", Command::DockRight),
            cmd("View: Toggle Sidebar", "⌘B", Command::ToggleSidebar),
            cmd("View: Light Theme", "", Command::ThemeLight),
            cmd("View: Dark Theme", "", Command::ThemeDark),
            cmd("View: Toggle Light/Dark Theme", "⌘⌥T", Command::ToggleTheme),
        ];
        for p in &self.config.profiles {
            items.push(Item {
                label: format!("Terminal: New {} Session", p.name),
                shortcut: String::new(),
                choice: Choice::Terminal(p.name.clone()),
            });
        }
        items.extend([
            cmd("Workspace: Trust This Folder", "", Command::TrustFolder),
            cmd("Preferences: Edit Global Config", "", Command::EditGlobalConfig),
            cmd("Preferences: Edit Project Config", "", Command::EditProjectConfig),
            cmd("Preferences: Reload Config", "", Command::ReloadConfig),
            cmd("Quit", "⌘Q", Command::Quit),
        ]);
        items
    }

    fn run_choice(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        match choice {
            Choice::File(path) => self.open_file(&path, window, cx),
            Choice::Terminal(name) => self.spawn_terminal(&name, window, cx),
            // Only used by prompts that own their palette (the git window).
            Choice::Index(0) => {
                if let Some(path) = self.pending_open.take() {
                    self.switch_folder(path, true, window, cx);
                }
            }
            Choice::Text(text) => match parse_goto(&text) {
                Some((line, col)) => {
                    if let Some(ed) = self.active_editor().cloned() {
                        ed.update(cx, |ed, cx| ed.goto(line, col, cx));
                    }
                    self.focus_main(window, cx);
                }
                None => self.flash(format!("“{text}” is not a line number"), cx),
            },
            Choice::Index(_) => self.pending_open = None,
            Choice::Command(c) => match c {
                Command::OpenFolder => self.prompt_open_folder(&OpenFolder, window, cx),
                Command::CloseFolder => self.close_folder(&CloseFolder, window, cx),
                Command::QuickOpen => self.open_palette(PaletteKind::Files, window, cx),
                Command::GotoLine if self.active_editor().is_some() => self.open_palette(PaletteKind::GotoLine, window, cx),
                Command::GotoLine => {}
                Command::Save => self.save(&Save, window, cx),
                Command::CloseTab => self.close_tab(self.active, window, cx),
                Command::NextTab => self.next_tab(&NextTab, window, cx),
                Command::PrevTab => self.prev_tab(&PrevTab, window, cx),
                Command::ToggleGit => self.set_git_visible(!self.show_git, window, cx),
                Command::ToggleTerminal => self.toggle_terminal(&ToggleTerminal, window, cx),
                Command::NewTerminal => self.new_terminal(&NewTerminal, window, cx),
                Command::CloseTerminal => self.close_session(self.active_term, false, window, cx),
                Command::DockBottom => self.dock_terminal(Dock::Bottom, window, cx),
                Command::DockLeft => self.dock_terminal(Dock::Left, window, cx),
                Command::DockRight => self.dock_terminal(Dock::Right, window, cx),
                Command::ToggleSidebar => self.toggle_sidebar(&ToggleSidebar, window, cx),
                Command::ThemeLight => self.set_theme(theme::Mode::Light, cx),
                Command::ThemeDark => self.set_theme(theme::Mode::Dark, cx),
                Command::ToggleTheme => self.toggle_theme(&ToggleTheme, window, cx),
                Command::TrustFolder => self.trust_folder(window, cx),
                Command::EditGlobalConfig => self.edit_config(false, window, cx),
                Command::EditProjectConfig => self.edit_config(true, window, cx),
                Command::ReloadConfig => {
                    self.load_config(cx);
                    if self.message.as_deref().is_none_or(|m| !m.starts_with("Config error")) {
                        self.flash("Config reloaded", cx);
                    }
                }
                Command::Quit => self.quit(&Quit, window, cx),
            },
        }
    }

    fn quit(&mut self, _: &Quit, _: &mut Window, cx: &mut Context<Self>) {
        let dirty = self.tabs.iter().filter(|t| t.read(cx).buffer.is_dirty()).count();
        if dirty > 0 && !self.pending_quit {
            self.pending_quit = true;
            self.flash(format!("{dirty} unsaved file(s): press ⌘Q again to quit and discard changes"), cx);
            return;
        }
        cx.quit();
    }

    // ---- sidebar -----------------------------------------------------------------------

    fn click_row(&mut self, path: PathBuf, is_dir: bool, window: &mut Window, cx: &mut Context<Self>) {
        if is_dir {
            if let Some(Err(e)) = self.tree.as_mut().map(|t| t.toggle(&path)) {
                self.flash(format!("Cannot read {}: {e}", path.display()), cx);
            }
            cx.notify();
        } else {
            self.open_file(&path, window, cx);
        }
    }

    // ---- rendering -----------------------------------------------------------------------

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active_path = self.active_editor().and_then(|e| e.read(cx).path.clone());
        let rows = self.tree.as_ref().map(|t| t.rows()).unwrap_or_default();
        div()
            .w(px(self.layout.sidebar_width))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(rgb(ui().sidebar_bg))
            .text_size(px(13.))
            .text_color(rgb(ui().sidebar_text))
            .child(
                div()
                    .h(px(30.))
                    .flex_none()
                    .px(px(13.))
                    .pt(px(10.))
                    .text_size(px(11.))
                    .text_color(rgb(ui().sidebar_header))
                    .child("FOLDERS"),
            )
            .child(
                div()
                    .id("tree")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows.into_iter().enumerate().map(|(i, row)| {
                        let selected = active_path.as_deref() == Some(row.path.as_path());
                        let icon = if row.is_dir {
                            if row.expanded { "icons/folder-open.svg" } else { "icons/folder.svg" }
                        } else {
                            file_icon(&row.name)
                        };
                        let arrow = if !row.is_dir {
                            None
                        } else if row.expanded {
                            Some("icons/chevron-down.svg")
                        } else {
                            Some("icons/chevron-right.svg")
                        };
                        let icon_color = if row.ignored { ui().icon_dim } else { ui().icon };
                        let (path, is_dir) = (row.path.clone(), row.is_dir);
                        div()
                            .id(("row", i))
                            .h(px(ROW_HEIGHT))
                            .flex_none()
                            .flex()
                            .items_center()
                            .pl(px(6. + row.depth as f32 * 11.))
                            .pr(px(6.))
                            .when(selected, |d| d.bg(rgb(ui().sidebar_selected)))
                            .when(!selected, |d| d.hover(|h| h.bg(rgb(ui().sidebar_hover))))
                            .when(row.ignored, |d| d.text_color(rgb(ui().sidebar_dim)))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.click_row(path.clone(), is_dir, window, cx)
                            }))
                            .child(
                                div().w(px(12.)).h(px(12.)).flex_none().when_some(arrow, |d, a| {
                                    d.child(svg().path(a).size(px(12.)).text_color(rgb(ui().icon)))
                                }),
                            )
                            .child(div().w(px(4.)).flex_none())
                            .child(svg().path(icon).size(px(16.)).flex_none().text_color(rgb(icon_color)))
                            .child(div().w(px(5.)).flex_none())
                            .child(div().overflow_hidden().whitespace_nowrap().child(row.name.clone()))
                    })),
            )
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(25.))
            .flex_none()
            .flex()
            .items_end()
            .bg(rgb(ui().tabbar_bg))
            .text_size(px(12.))
            .children(self.tabs.iter().enumerate().map(|(i, t)| {
                let ed = t.read(cx);
                let active = i == self.active;
                let dirty = ed.buffer.is_dirty();
                let fg = if active { ui().tab_active_text } else { ui().tab_inactive_text };
                div()
                    .id(("tab", i))
                    .group("tab")
                    .h(px(23.))
                    .pl(px(12.))
                    .pr(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .when(active, |d| d.bg(rgb(ui().tab_active_bg)).italic())
                    .text_color(rgb(fg))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.activate(i, window, cx)))
                    .child(ed.title())
                    .child(
                        // Close button: always on the active tab, on hover for the others.
                        // A dirty tab shows ● until hovered, like Sublime.
                        div()
                            .id(("tab-close", i))
                            .w(px(14.))
                            .h(px(14.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .text_size(px(12.))
                            .when(!active && !dirty, |d| d.invisible().group_hover("tab", |g| g.visible()))
                            .hover(|h| h.bg(rgb(ui().tab_close_hover_bg)).text_color(rgb(ui().tab_close_hover_text)))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(i, window, cx)
                            }))
                            .child(if dirty { "●" } else { "×" }),
                    )
            }))
    }

    fn render_terminal_pane(&self, cx: &mut Context<Self>) -> gpui::Div {
        let active = &self.terminals[self.active_term.min(self.terminals.len() - 1)];
        let dock = self.layout.terminal_dock;
        let pane = div().flex_none().flex().flex_col().bg(rgb(ui().term_bg)).border_color(rgb(ui().pane_border));
        let pane = match dock {
            Dock::Bottom => pane.h(px(self.layout.terminal_height)).w_full().border_t_1(),
            Dock::Left => pane.w(px(self.layout.terminal_width)).h_full().border_r_1(),
            Dock::Right => pane.w(px(self.layout.terminal_width)).h_full().border_l_1(),
        };
        pane
            .child(
                div()
                    .id("term-strip")
                    .cursor(CursorStyle::OpenHand)
                    // Drag the header onto a highlighted edge to dock the panel there.
                    .on_drag(DraggedPanel, |_, _, _, cx| cx.new(|_| DragGhost("Terminal".into())))
                    .h(px(24.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .px(px(6.))
                    .bg(rgb(ui().strip_bg))
                    .text_size(px(11.))
                    .text_color(rgb(ui().strip_text))
                    .children(self.terminals.iter().enumerate().map(|(i, s)| {
                        let is_active = i == self.active_term;
                        let title = s.view.read(cx).title().to_owned();
                        let exited = s.view.read(cx).exited.is_some();
                        div()
                            .id(("term-tab", i))
                            .h(px(20.))
                            .pl(px(10.))
                            .pr(px(4.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .rounded(px(4.))
                            .when(is_active, |d| d.bg(rgb(ui().strip_active_bg)).text_color(rgb(ui().strip_active_text)))
                            .when(!is_active, |d| d.hover(|h| h.bg(rgb(ui().strip_hover))))
                            .when(exited, |d| d.italic())
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.active_term = i;
                                let h = this.terminals[i].view.read(cx).focus_handle(cx);
                                window.focus(&h);
                                cx.notify();
                            }))
                            .child(s.name.clone())
                            .when(!title.is_empty() && is_active, |d| {
                                d.child(div().max_w(px(260.)).truncate().text_color(rgb(ui().strip_dim)).child(title))
                            })
                            .child(
                                div()
                                    .id(("term-close", i))
                                    .w(px(14.))
                                    .h(px(14.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.))
                                    .hover(|h| h.bg(rgb(ui().strip_close_hover)))
                                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                        cx.stop_propagation();
                                        this.close_session(i, false, window, cx)
                                    }))
                                    .child("×"),
                            )
                    }))
                    .child(
                        div()
                            .id("term-new")
                            .w(px(22.))
                            .h(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .text_size(px(14.))
                            .hover(|h| h.bg(rgb(ui().strip_hover)))
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.new_terminal(&NewTerminal, window, cx)
                            }))
                            .child("+"),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("term-dock")
                            .h(px(20.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .rounded(px(4.))
                            .text_size(px(10.5))
                            .hover(|h| h.bg(rgb(ui().strip_hover)))
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                let next = this.layout.terminal_dock.next();
                                this.dock_terminal(next, window, cx)
                            }))
                            .child(format!("⇆ {}", dock.label())),
                    ),
            )
            .child(div().flex_1().min_h_0().min_w_0().child(active.view.clone()))
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> gpui::Div {
        let button = div()
            .id("open-folder")
            .px(px(22.))
            .h(px(34.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .cursor_pointer()
            .bg(rgb(ui().accent))
            .hover(|h| h.bg(rgb(ui().accent_hover)))
            .text_color(rgb(ui().on_accent))
            .text_size(px(14.))
            .font_weight(FontWeight::MEDIUM)
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.prompt_open_folder(&OpenFolder, window, cx)
            }))
            .child("Open Folder…");

        let recents = self.recent.iter().enumerate().map(|(i, p)| {
            let path = p.clone();
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let parent = p.parent().map(|d| d.display().to_string()).unwrap_or_default();
            div()
                .id(("recent", i))
                .w_full()
                .h(px(32.))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(8.))
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|h| h.bg(rgb(ui().welcome_hover)))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.open_folder(path.clone(), window, cx)
                }))
                .child(svg().path("icons/folder.svg").size(px(16.)).flex_none().text_color(rgb(ui().welcome_icon)))
                .child(div().text_color(rgb(ui().welcome_title)).child(name))
                .child(div().truncate().text_size(px(11.)).text_color(rgb(ui().welcome_muted)).child(parent))
        });

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(20.))
            .bg(rgb(ui().welcome_bg))
            .text_color(rgb(ui().welcome_text))
            .child(div().text_size(px(30.)).font_weight(FontWeight::LIGHT).text_color(rgb(ui().welcome_title)).child("mide"))
            .child(button)
            .child(div().text_size(px(12.)).child("⌘O to open a folder · its git history is picked up automatically (⌘⇧G)"))
            .when(!self.recent.is_empty(), |d| {
                d.child(
                    div()
                        .w(px(460.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .text_size(px(13.))
                        .child(
                            div()
                                .px(px(10.))
                                .pb(px(4.))
                                .text_size(px(11.))
                                .text_color(rgb(ui().welcome_muted))
                                .child("RECENT"),
                        )
                        .children(recents),
                )
            })
    }

    fn status_text(&self, cx: &App) -> (String, String) {
        if self.root.is_none() {
            let (pos, syntax) = match self.active_editor().map(|e| e.read(cx)) {
                Some(ed) => {
                    let (l, c) = ed.caret_line_col();
                    (format!(", Line {l}, Column {c}"), ed.syntax_name.clone())
                }
                None => (String::new(), String::new()),
            };
            let left = match &self.message {
                Some(m) => m.to_string(),
                None => format!("No folder open{pos}"),
            };
            return (left, syntax);
        }
        let git = match &self.git {
            Some(g) => format!(
                "Git branch: {}, index: {}, working: {}",
                g.branch.as_deref().unwrap_or("(none)"),
                if g.index_clean { "✓" } else { "✗" },
                if g.working_clean { "✓" } else { "✗" },
            ),
            None => "No git repository".to_owned(),
        };
        let Some(ed) = self.active_editor().map(|e| e.read(cx)) else {
            return (git, String::new());
        };
        let (line, col) = ed.caret_line_col();
        let left = match &self.message {
            Some(m) => m.to_string(),
            None => format!("{git}, Line {line}, Column {col}"),
        };
        let branch = self.git.as_ref().and_then(|g| g.branch.clone()).unwrap_or_default();
        (left, format!("{branch}      Spaces: 4      {}", ed.syntax_name))
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Window title: "<file> — <folder>", with the macOS edited dot.
        let folder = self
            .root
            .as_ref()
            .and_then(|r| r.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "mide".to_owned());
        let (title, edited) = match self.active_editor().map(|e| e.read(cx)) {
            Some(ed) => (format!("{} — {folder}", ed.title()), ed.buffer.is_dirty()),
            None => (folder, false),
        };
        if title != self.last_title {
            window.set_window_title(&title);
            self.last_title = title;
        }
        window.set_window_edited(edited);
        if !edited && self.pending_quit {
            // Nothing left to lose: a later ⌘Q should quit immediately.
            if self.tabs.iter().all(|t| !t.read(cx).buffer.is_dirty()) {
                self.pending_quit = false;
            }
        }

        let (left, right) = self.status_text(cx);
        let editor_area = match self.active_editor() {
            Some(ed) => div().flex_1().min_h_0().child(ed.clone()),
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(ui().editor_bg))
                .text_color(rgb(ui().empty_text))
                .text_size(px(13.))
                .child("Open a file from the sidebar"),
        };

        let term_visible = self.show_term && !self.terminals.is_empty();
        let dock = self.layout.terminal_dock;
        let main_area = match (&self.git_view, self.show_git) {
            _ if self.root.is_none() && self.tabs.is_empty() && !term_visible => self.render_welcome(cx),
            (Some(view), true) => div().flex_1().min_h_0().child(view.clone()),
            _ => {
                let mut editor_col = div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(self.render_tabs(cx))
                    .child(editor_area);
                if term_visible && dock == Dock::Bottom {
                    editor_col = editor_col
                        .child(self.resize_handle("rs-term-bottom", ResizeTarget::TerminalBottom, cx))
                        .child(self.render_terminal_pane(cx));
                }
                let mut row = div().flex_1().min_h_0().flex();
                if self.layout.show_sidebar && self.tree.is_some() {
                    row = row
                        .child(self.render_sidebar(cx))
                        .child(self.resize_handle("rs-sidebar", ResizeTarget::Sidebar, cx));
                }
                if term_visible && dock == Dock::Left {
                    row = row
                        .child(self.render_terminal_pane(cx))
                        .child(self.resize_handle("rs-term-left", ResizeTarget::TerminalLeft, cx));
                }
                row = row.child(editor_col);
                if term_visible && dock == Dock::Right {
                    row = row
                        .child(self.resize_handle("rs-term-right", ResizeTarget::TerminalRight, cx))
                        .child(self.render_terminal_pane(cx));
                }
                row
            }
        };
        let dragging_panel = term_visible && cx.has_active_drag();

        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::close_active))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::toggle_git))
            .on_action(cx.listener(Self::prompt_open_folder))
            .on_action(cx.listener(|this, _: &QuickOpen, window, cx| this.open_palette(PaletteKind::Files, window, cx)))
            .on_action(cx.listener(|this, _: &CommandPalette, window, cx| {
                this.open_palette(PaletteKind::Commands, window, cx)
            }))
            .on_action(cx.listener(Self::toggle_terminal))
            .on_action(cx.listener(Self::new_terminal))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(|this, _: &GotoLine, window, cx| {
                if this.active_editor().is_some() {
                    this.open_palette(PaletteKind::GotoLine, window, cx);
                }
            }))
            .on_action(cx.listener(Self::close_folder))
            .on_mouse_move(cx.listener(Self::on_root_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_root_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_root_mouse_up))
            .font_family(".SystemUIFont")
            .child(main_area)
            .child(
                div()
                    .h(px(22.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(10.))
                    .bg(rgb(ui().status_bg))
                    .text_color(rgb(ui().status_text))
                    .text_size(px(11.))
                    .child(
                        div()
                            .id("status-left")
                            .cursor_pointer()
                            .hover(|h| h.text_color(rgb(ui().status_hover_text)))
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.set_git_visible(!this.show_git, window, cx)
                            }))
                            .child(left),
                    )
                    .child(right),
            )
            .when(dragging_panel, |d| d.child(self.render_drop_zones(cx)))
            .children(self.palette.clone().map(|palette| {
                // Full-window backdrop: clicking outside the palette dismisses it.
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.close_palette(true);
                            this.refocus_after_palette(window, cx);
                            cx.notify();
                        }),
                    )
                    .child(div().child(palette))
            }))
    }
}

/// `file:///Users/me/My%20Project` -> `/Users/me/My Project`. Plain paths pass through.
/// `42`, `42:7` or `42,7` -> (line, column), both 1-based; the column defaults to 1.
fn parse_goto(text: &str) -> Option<(usize, usize)> {
    let mut parts = text.trim().split([':', ',']).map(str::trim);
    let line: usize = parts.next()?.parse().ok()?;
    let col: usize = match parts.next() {
        Some(c) if !c.is_empty() => c.parse().ok()?,
        _ => 1,
    };
    parts.next().is_none().then_some((line.max(1), col.max(1)))
}

pub fn url_to_path(url: &str) -> Option<PathBuf> {
    let raw = match url.strip_prefix("file://") {
        Some(rest) => rest.strip_prefix("localhost").unwrap_or(rest),
        None if url.starts_with('/') => url,
        None => return None,
    };
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
            if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out).into_owned()))
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_to_line_input_parsing() {
        assert_eq!(parse_goto("42"), Some((42, 1)));
        assert_eq!(parse_goto(" 42:7 "), Some((42, 7)));
        assert_eq!(parse_goto("12,3"), Some((12, 3)));
        assert_eq!(parse_goto("0"), Some((1, 1)));
        assert_eq!(parse_goto("5:"), Some((5, 1)));
        assert_eq!(parse_goto("abc"), None);
        assert_eq!(parse_goto("1:2:3"), None);
        assert_eq!(parse_goto(""), None);
    }

    #[test]
    fn file_urls_become_paths() {
        // A multi-byte char right after '%' must not panic.
        assert_eq!(url_to_path("file:///tmp/%é"), Some(PathBuf::from("/tmp/%é")));
        assert_eq!(url_to_path("file:///Users/me/My%20Project/"), Some(PathBuf::from("/Users/me/My Project/")));
        assert_eq!(url_to_path("file://localhost/tmp/a%2Bb.rs"), Some(PathBuf::from("/tmp/a+b.rs")));
        assert_eq!(url_to_path("/already/a/path"), Some(PathBuf::from("/already/a/path")));
        assert_eq!(url_to_path("https://example.com"), None);
        // Malformed escapes are kept literally rather than dropped.
        assert_eq!(url_to_path("file:///tmp/100%"), Some(PathBuf::from("/tmp/100%")));
        assert_eq!(url_to_path("file:///tmp/%zz"), Some(PathBuf::from("/tmp/%zz")));
        assert_eq!(url_to_path("file:///tmp/caf%C3%A9"), Some(PathBuf::from("/tmp/café")));
    }
}
