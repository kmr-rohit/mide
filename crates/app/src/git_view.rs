//! Git window, modeled on Sublime Merge / SourceTree: Locations on the left, the
//! commit list with a graph rail in the middle, commit details with side-by-side
//! diffs on the right, a toolbar of actions on top, and a working-copy view with a
//! commit box when "uncommitted changes" is selected.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{Datelike, Local, TimeZone};
use gpui::{
    actions, canvas, div, point, prelude::*, px, quad, rgb, size, svg, uniform_list, App, BorderStyle,
    Bounds, ClickEvent, ClipboardItem, Context, CursorStyle, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, Hsla, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder,
    Pixels, Render, ScrollStrategy, SharedString, UniformListScrollHandle, WeakEntity, Window,
};
use mide_core::Buffer;
use mide_fuzzy::Kind;
use mide_git::diff::{side_by_side, Cell, SideBySideRow};
use mide_git::graph::GraphRow;
use mide_git::ops::{self, OpResult};
use mide_git::{FileDiff, FileStatus, LineKind, LogEntry, Oid, RefKind, RefLabel, Refs, Repo};
use mide_syntax::{Syntaxes, Theme};

use crate::editor::EditorView;
use crate::fonts;
use crate::palette::{Choice, Item, Palette, PaletteEvent};
use crate::theme::ui;

actions!(
    git_view,
    [CloseGit, PrevCommit, NextCommit, PagePrevCommit, PageNextCommit, FirstCommit, LastCommit, DoCommit]
);

pub enum GitEvent {
    Close,
    /// Open this project-relative path in the editor.
    OpenFile(PathBuf),
    /// Repository state changed (an operation ran); refresh status elsewhere.
    Refreshed,
    /// The user resized a column: (locations width, commits width).
    LayoutChanged(f32, f32),
}


const LOG_LIMIT: usize = 2000;
const ROW_H: f32 = 40.0;
const DETAIL_ROW_H: f32 = 18.0;
const LANE_W: f32 = 12.0;
const MAX_LANES: usize = 12;
const MAX_DIFF_ROWS_PER_FILE: usize = 1500;
const MIN_PANE_W: f32 = 120.0;

fn rgba(v: u32) -> Hsla {
    rgb(v).into()
}

/// Everything read from the repository, gathered off the UI thread.
#[allow(clippy::large_enum_variant)] // built once per reload, moved once
enum Snapshot {
    Failed(String),
    Loaded {
        refs: Refs,
        log: Vec<LogEntry>,
        entries: Vec<mide_git::StatusEntry>,
        summary: Option<mide_git::StatusSummary>,
    },
}

fn load_snapshot(root: &Path) -> Snapshot {
    match Repo::discover(root) {
        Err(e) => Snapshot::Failed(e.message().to_owned()),
        Ok(mut repo) => Snapshot::Loaded {
            refs: repo.refs().unwrap_or_default(),
            log: repo.log(0, LOG_LIMIT).unwrap_or_default(),
            entries: repo.status().unwrap_or_default(),
            summary: repo.status_summary().ok(),
        },
    }
}

// ---- model ---------------------------------------------------------------------

/// Something a button can do.
#[derive(Clone, Debug)]
enum GitAction {
    Stage(String),
    Unstage(String),
    /// path, is-untracked
    Discard(String, bool),
    StageAll,
    UnstageAll,
    CheckoutCommit(String),
    BranchHere(String),
    CherryPick(String),
    Revert(String),
    CopyHash(String),
}

type Buttons = Vec<(&'static str, GitAction)>;

#[derive(Clone)]
enum DetailRow {
    Meta { label: &'static str, value: String, mono: bool, badges: Vec<RefLabel> },
    Message(String),
    Gap,
    Title { text: String, buttons: Buttons },
    Actions(Buttons),
    File { path: String, status: FileStatus, adds: usize, dels: usize, openable: bool, buttons: Buttons },
    Hunk(String),
    Diff(SideBySideRow),
    Notice(String),
}

#[derive(Clone)]
enum Confirm {
    Discard { path: String, untracked: bool },
    StageAllThenCommit,
}

enum Prompt {
    NewBranch { from: Option<String> },
    Confirm(Confirm),
}

struct Notice {
    text: String,
    error: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Locations,
    Commits,
}

pub struct GitView {
    root: PathBuf,
    repo: Option<Repo>,
    error: Option<String>,
    refs: Refs,
    log: Vec<LogEntry>,
    dirty_files: usize,
    staged_files: usize,
    unstaged_files: usize,
    branch: Option<String>,
    ahead: usize,
    behind: usize,
    lanes: usize,
    selected: usize,
    /// The rows as built from the selection.
    detail: Vec<DetailRow>,
    /// What the list shows: `detail`, with long diff lines wrapped when `wrap` is on.
    shown: Vec<DetailRow>,
    /// Wrap long diff lines onto extra rows (toggle in the toolbar).
    wrap: bool,
    /// Characters that fit in one half of the diff at the current window width.
    wrap_cols: usize,
    /// Who commits are made as (the active profile), shown in the toolbar.
    identity: Option<String>,
    collapsed: HashSet<&'static str>,
    commits_scroll: UniformListScrollHandle,
    detail_scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
    message_box: Entity<EditorView>,
    amend: bool,
    busy: Option<String>,
    notice: Option<Notice>,
    prompt: Option<(Entity<Palette>, Prompt)>,
    locations_w: f32,
    commits_w: f32,
    /// Active column drag: (which, mouse x at start, width at start).
    resizing: Option<(Pane, f32, f32)>,
    loading: bool,
    load_gen: u64,
}

impl EventEmitter<GitEvent> for GitView {}

impl Focusable for GitView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl GitView {
    pub fn new(
        root: PathBuf,
        syntaxes: Arc<Syntaxes>,
        theme: Arc<Theme>,
        widths: (f32, f32),
        cx: &mut Context<Self>,
    ) -> Self {
        let message_box = cx.new(|cx| {
            let mut ed = EditorView::new(Buffer::default(), None, syntaxes, theme, cx);
            ed.show_gutter = false;
            ed
        });
        let mut v = Self {
            root,
            repo: None,
            error: None,
            refs: Refs::default(),
            log: Vec::new(),
            dirty_files: 0,
            staged_files: 0,
            unstaged_files: 0,
            branch: None,
            ahead: 0,
            behind: 0,
            lanes: 1,
            selected: 0,
            detail: Vec::new(),
            shown: Vec::new(),
            wrap: true,
            wrap_cols: 0,
            identity: None,
            collapsed: ["TAGS", "STASHES", "SUBMODULES", "WORKTREES"].into_iter().collect(),
            commits_scroll: UniformListScrollHandle::new(),
            detail_scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            message_box,
            amend: false,
            busy: None,
            notice: None,
            prompt: None,
            locations_w: widths.0.max(MIN_PANE_W),
            commits_w: widths.1.max(MIN_PANE_W),
            resizing: None,
            loading: false,
            load_gen: 0,
        };
        v.reload(cx);
        v
    }

    fn has_wt(&self) -> bool {
        self.dirty_files > 0
    }

    fn list_len(&self) -> usize {
        self.log.len() + self.has_wt() as usize
    }

    fn working_copy_selected(&self) -> bool {
        self.has_wt() && self.selected == 0
    }

    /// Re-read everything on a background thread (a big repo's log and status can
    /// take seconds), then apply it, keeping the selected commit if it still exists.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.load_gen += 1;
        let generation = self.load_gen;
        self.loading = true;
        let root = self.root.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let snap = cx.background_executor().spawn(async move { load_snapshot(&root) }).await;
            let _ = this.update(cx, |v, cx| v.apply_snapshot(generation, snap, cx));
        })
        .detach();
        cx.notify();
    }

    fn apply_snapshot(&mut self, generation: u64, snap: Snapshot, cx: &mut Context<Self>) {
        if generation != self.load_gen {
            return; // a newer reload is already on its way
        }
        self.loading = false;
        let keep = self.selected_commit_id();
        let was_wt = self.working_copy_selected();
        match snap {
            Snapshot::Loaded { refs, log, entries, summary } => {
                self.refs = refs;
                self.log = log;
                self.dirty_files = entries.len();
                self.staged_files = entries.iter().filter(|e| e.index.is_some()).count();
                self.unstaged_files = entries.iter().filter(|e| e.worktree.is_some()).count();
                if let Some(s) = summary {
                    self.branch = s.branch;
                    self.ahead = s.ahead;
                    self.behind = s.behind;
                }
                self.repo = Repo::discover(&self.root).ok();
                self.error = None;
            }
            Snapshot::Failed(e) => {
                self.repo = None;
                self.error = Some(e);
                self.log.clear();
                self.dirty_files = 0;
            }
        }
        self.lanes = self.log.iter().map(|e| e.graph.width).max().unwrap_or(1).clamp(1, MAX_LANES);
        let wt = self.has_wt() as usize;
        let ix = if was_wt && wt == 1 {
            0
        } else {
            keep.and_then(|id| self.log.iter().position(|e| e.commit.id == id).map(|i| i + wt)).unwrap_or(0)
        };
        self.select(ix, false, cx);
    }

    fn selected_commit_id(&self) -> Option<Oid> {
        let wt = self.has_wt() as usize;
        self.selected.checked_sub(wt).and_then(|i| self.log.get(i)).map(|e| e.commit.id)
    }

    /// Rebuild `shown` from `detail` for the current wrap setting and width.
    fn rewrap(&mut self) {
        self.shown = if self.wrap && self.wrap_cols > 8 {
            wrap_detail(&self.detail, self.wrap_cols)
        } else {
            self.detail.clone()
        };
    }

    pub fn set_identity(&mut self, identity: Option<String>, cx: &mut Context<Self>) {
        if self.identity != identity {
            self.identity = identity;
            cx.notify();
        }
    }

    fn toggle_wrap(&mut self, cx: &mut Context<Self>) {
        self.wrap = !self.wrap;
        self.rewrap();
        cx.notify();
    }

    fn select(&mut self, ix: usize, scroll: bool, cx: &mut Context<Self>) {
        let len = self.list_len();
        self.selected = if len == 0 { 0 } else { ix.min(len - 1) };
        self.detail = self.build_detail();
        self.rewrap();
        self.detail_scroll.scroll_to_item(0, ScrollStrategy::Top);
        if scroll && len > 0 {
            self.commits_scroll.scroll_to_item(self.selected, ScrollStrategy::Center);
        }
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let ix = (self.selected as isize + delta).clamp(0, self.list_len().max(1) as isize - 1) as usize;
        self.select(ix, true, cx);
    }

    fn jump_to_ref(&mut self, name: &str, cx: &mut Context<Self>) {
        let wt = self.has_wt() as usize;
        if let Some(i) = self.log.iter().position(|e| e.commit.labels.iter().any(|l| l.name == name)) {
            self.select(i + wt, true, cx);
        }
    }

    // ---- running operations -------------------------------------------------------------

    fn say(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.notice = Some(Notice { text: text.into(), error });
        cx.notify();
    }

    /// Run a git operation on a background thread, then show its result and reload.
    fn exec(
        &mut self,
        label: &str,
        clear_message: bool,
        cx: &mut Context<Self>,
        op: impl FnOnce(&Path) -> OpResult + Send + 'static,
    ) {
        if let Some(running) = &self.busy {
            let msg = format!("Wait for “{running}” to finish first");
            self.say(msg, true, cx);
            return;
        }
        let label = label.to_owned();
        self.busy = Some(label.clone());
        self.notice = None;
        let root = self.root.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let result = cx.background_executor().spawn(async move { op(&root) }).await;
            let _ = this.update(cx, |v, cx| v.finish(&label, clear_message, result, cx));
        })
        .detach();
        cx.notify();
    }

    fn finish(&mut self, label: &str, clear_message: bool, result: OpResult, cx: &mut Context<Self>) {
        self.busy = None;
        match result {
            Ok(out) => {
                let tail: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).take(8).collect();
                let text = if tail.is_empty() { format!("{label}: done") } else { format!("{label}: done\n{}", tail.join("\n")) };
                self.say(text, false, cx);
                if clear_message {
                    self.message_box.update(cx, |ed, cx| ed.clear(cx));
                    self.amend = false;
                }
            }
            Err(e) => {
                let lines: Vec<&str> = e.lines().take(10).collect();
                self.say(format!("{label} failed:\n{}", lines.join("\n")), true, cx);
            }
        }
        self.reload(cx);
        cx.emit(GitEvent::Refreshed);
    }

    fn do_pull(&mut self, cx: &mut Context<Self>) {
        self.exec("Pull", false, cx, ops::pull);
    }

    fn do_push(&mut self, cx: &mut Context<Self>) {
        self.exec("Push", false, cx, ops::push);
    }

    fn do_fetch(&mut self, cx: &mut Context<Self>) {
        self.exec("Fetch", false, cx, ops::fetch);
    }

    fn do_stash(&mut self, cx: &mut Context<Self>) {
        self.exec("Stash", false, cx, |r| ops::stash_push(r, ""));
    }

    fn do_stash_pop(&mut self, cx: &mut Context<Self>) {
        self.exec("Pop stash", false, cx, ops::stash_pop);
    }

    fn focus_commit_box(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.has_wt() {
            self.select(0, true, cx);
            let h = self.message_box.read(cx).focus_handle(cx);
            window.focus(&h);
        } else {
            self.say("Nothing to commit: the working tree is clean", false, cx);
        }
    }

    fn do_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let message = self.message_box.read(cx).text();
        if self.staged_files == 0 && !self.amend {
            if self.unstaged_files > 0 {
                self.open_confirm(
                    "Nothing is staged. Stage all changes and commit?",
                    "Yes, stage everything and commit",
                    Confirm::StageAllThenCommit,
                    window,
                    cx,
                );
            } else {
                self.say("Nothing to commit", false, cx);
            }
            return;
        }
        if message.trim().is_empty() && !self.amend {
            self.say("Enter a commit message first", true, cx);
            return;
        }
        let amend = self.amend;
        self.exec(if amend { "Amend commit" } else { "Commit" }, true, cx, move |r| ops::commit(r, &message, amend));
    }

    fn toggle_amend(&mut self, cx: &mut Context<Self>) {
        self.amend = !self.amend;
        // Amending with an empty box starts from the previous message, like SourceTree.
        if self.amend && self.message_box.read(cx).text().trim().is_empty() {
            if let Some(prev) = self.log.first().map(|e| e.commit.message.trim_end().to_owned()) {
                self.message_box.update(cx, |ed, cx| ed.set_text(&prev, cx));
            }
        }
        cx.notify();
    }

    fn run_action(&mut self, action: GitAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            GitAction::Stage(p) => self.exec("Stage", false, cx, move |r| ops::stage(r, &[p])),
            GitAction::Unstage(p) => self.exec("Unstage", false, cx, move |r| ops::unstage(r, &[p])),
            GitAction::StageAll => self.exec("Stage all", false, cx, ops::stage_all),
            GitAction::UnstageAll => self.exec("Unstage all", false, cx, ops::unstage_all),
            GitAction::Discard(path, untracked) => {
                let what = if untracked { "Delete" } else { "Discard changes to" };
                self.open_confirm(
                    &format!("This cannot be undone: {what} {path}?"),
                    &format!("Yes, {} {path}", what.to_lowercase()),
                    Confirm::Discard { path, untracked },
                    window,
                    cx,
                );
            }
            GitAction::CheckoutCommit(rev) => self.exec("Checkout", false, cx, move |r| ops::checkout_detached(r, &rev)),
            GitAction::BranchHere(rev) => self.open_new_branch(Some(rev), window, cx),
            GitAction::CherryPick(rev) => self.exec("Cherry-pick", false, cx, move |r| ops::cherry_pick(r, &rev)),
            GitAction::Revert(rev) => self.exec("Revert", false, cx, move |r| ops::revert(r, &rev)),
            GitAction::CopyHash(rev) => {
                cx.write_to_clipboard(ClipboardItem::new_string(rev.clone()));
                self.say(format!("Copied {}", &rev[..rev.len().min(12)]), false, cx);
            }
        }
    }

    fn switch_to(&mut self, name: &str, cx: &mut Context<Self>) {
        // A remote branch like `origin/feature` is switched to by its local name,
        // which makes git create a tracking branch.
        let local = name.split_once('/').map_or(name, |(_, rest)| rest);
        let target = if self.refs.branches.iter().any(|b| b == name) { name.to_owned() } else { local.to_owned() };
        self.exec("Switch branch", false, cx, move |r| ops::switch_branch(r, &target));
    }

    // ---- prompts ----------------------------------------------------------------------------

    fn open_prompt(&mut self, prompt: Prompt, palette: Entity<Palette>, window: &mut Window, cx: &mut Context<Self>) {
        cx.subscribe_in(&palette, window, |this, _, ev: &PaletteEvent, window, cx| {
            let Some((_, prompt)) = this.prompt.take() else { return };
            match ev {
                PaletteEvent::Dismissed => {
                    window.focus(&this.focus_handle);
                }
                PaletteEvent::Chosen(choice) => {
                    window.focus(&this.focus_handle);
                    this.run_prompt(prompt, choice.clone(), window, cx);
                }
            }
            cx.notify();
        })
        .detach();
        window.focus(&palette.read(cx).focus_handle(cx));
        self.prompt = Some((palette, prompt));
        cx.notify();
    }

    fn open_new_branch(&mut self, from: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = match &from {
            Some(rev) => format!("New branch name (from {})…", &rev[..rev.len().min(8)]),
            None => "New branch name (from current commit)…".to_owned(),
        };
        let palette = cx.new(|cx| Palette::input(&placeholder, cx));
        self.open_prompt(Prompt::NewBranch { from }, palette, window, cx);
    }

    fn open_confirm(&mut self, question: &str, yes: &str, what: Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let items = vec![
            Item { label: yes.to_owned(), shortcut: "↵".into(), choice: Choice::Index(0) },
            Item { label: "Cancel".into(), shortcut: "esc".into(), choice: Choice::Index(1) },
        ];
        let palette = cx.new(|cx| Palette::new(items, Kind::Text, question, cx));
        self.open_prompt(Prompt::Confirm(what), palette, window, cx);
    }

    fn run_prompt(&mut self, prompt: Prompt, choice: Choice, _window: &mut Window, cx: &mut Context<Self>) {
        match (prompt, choice) {
            (Prompt::NewBranch { from }, Choice::Text(name)) => {
                self.exec("Create branch", false, cx, move |r| ops::create_branch(r, &name, from.as_deref()));
            }
            (Prompt::Confirm(Confirm::Discard { path, untracked }), Choice::Index(0)) => {
                self.exec("Discard", false, cx, move |r| ops::discard(r, &path, untracked));
            }
            (Prompt::Confirm(Confirm::StageAllThenCommit), Choice::Index(0)) => {
                let message = self.message_box.read(cx).text();
                if message.trim().is_empty() {
                    self.say("Enter a commit message first", true, cx);
                } else {
                    self.exec("Commit", true, cx, move |r| {
                        ops::stage_all(r)?;
                        ops::commit(r, &message, false)
                    });
                }
            }
            _ => {}
        }
    }

    // ---- resizing ----------------------------------------------------------------------------

    fn start_resize(&mut self, pane: Pane, e: &MouseDownEvent) {
        let w = if pane == Pane::Locations { self.locations_w } else { self.commits_w };
        self.resizing = Some((pane, f32::from(e.position.x), w));
    }

    fn on_mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((pane, x0, w0)) = self.resizing else { return };
        let w = (w0 + f32::from(e.position.x) - x0).clamp(MIN_PANE_W, 700.0);
        match pane {
            Pane::Locations => self.locations_w = w,
            Pane::Commits => self.commits_w = w,
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.resizing.take().is_some() {
            cx.emit(GitEvent::LayoutChanged(self.locations_w, self.commits_w));
        }
    }

    // ---- detail building -------------------------------------------------------------

    fn build_detail(&self) -> Vec<DetailRow> {
        let Some(repo) = &self.repo else { return Vec::new() };
        let wt = self.has_wt() as usize;
        let mut rows = Vec::new();
        if wt == 1 && self.selected == 0 {
            for staged in [false, true] {
                let (title, bulk) = if staged {
                    ("Staged changes", ("Unstage all", GitAction::UnstageAll))
                } else {
                    ("Unstaged changes", ("Stage all", GitAction::StageAll))
                };
                match repo.working_diff(staged) {
                    Ok(files) if !files.is_empty() => {
                        rows.push(DetailRow::Title { text: format!("{title} ({} files)", files.len()), buttons: vec![bulk] });
                        push_files(&mut rows, &files, |f| {
                            let path = f.display_path().to_owned();
                            if staged {
                                vec![("Unstage", GitAction::Unstage(path))]
                            } else {
                                vec![
                                    ("Stage", GitAction::Stage(path.clone())),
                                    ("Discard", GitAction::Discard(path, f.status == FileStatus::Added)),
                                ]
                            }
                        });
                        rows.push(DetailRow::Gap);
                    }
                    Ok(_) => {}
                    Err(e) => rows.push(DetailRow::Notice(format!("{title}: {e}"))),
                }
            }
            return rows;
        }
        let Some(entry) = self.selected.checked_sub(wt).and_then(|i| self.log.get(i)) else {
            return rows;
        };
        let detail = match repo.commit_detail(entry.commit.id) {
            Ok(d) => d,
            Err(e) => return vec![DetailRow::Notice(format!("Cannot read commit: {e}"))],
        };
        let info = &detail.info;
        let hash = info.id.to_string();
        let mut actions: Buttons = vec![
            ("Checkout", GitAction::CheckoutCommit(hash.clone())),
            ("New Branch…", GitAction::BranchHere(hash.clone())),
        ];
        if info.parents.len() <= 1 {
            actions.push(("Cherry-pick", GitAction::CherryPick(hash.clone())));
            actions.push(("Revert", GitAction::Revert(hash.clone())));
        }
        actions.push(("Copy Hash", GitAction::CopyHash(hash.clone())));
        rows.push(DetailRow::Actions(actions));
        let meta = |label, value: String, mono| DetailRow::Meta { label, value, mono, badges: Vec::new() };
        rows.push(meta("Commit Hash", hash, true));
        rows.push(meta("Tree", detail.tree.to_string(), true));
        rows.push(meta("Author", format!("{} <{}>", info.author_name, info.author_email), false));
        rows.push(meta("Date", fmt_detail_date(info.time), false));
        rows.push(meta(
            "Parents",
            info.parents.iter().map(|p| p.to_string()[..7].to_owned()).collect::<Vec<_>>().join(", "),
            true,
        ));
        let branches: Vec<RefLabel> =
            info.labels.iter().filter(|l| l.kind != RefKind::Tag).cloned().collect();
        let tags: Vec<RefLabel> = info.labels.iter().filter(|l| l.kind == RefKind::Tag).cloned().collect();
        if !branches.is_empty() {
            rows.push(DetailRow::Meta { label: "Branches", value: String::new(), mono: false, badges: branches });
        }
        if !tags.is_empty() {
            rows.push(DetailRow::Meta { label: "Tags", value: String::new(), mono: false, badges: tags });
        }
        rows.push(meta(
            "Stats",
            format!("{} files changed: −{} +{}", detail.files.len(), detail.deletions, detail.additions),
            false,
        ));
        rows.push(DetailRow::Gap);
        for line in info.message.trim_end().lines() {
            rows.push(DetailRow::Message(line.to_owned()));
        }
        rows.push(DetailRow::Gap);
        push_files(&mut rows, &detail.files, |_| Vec::new());
        rows
    }

    // ---- rendering pieces -------------------------------------------------------------

    fn tool_button(
        &self,
        id: &'static str,
        label: &str,
        primary: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let disabled = self.busy.is_some();
        div()
            .id(id)
            .h(px(24.))
            .px(px(10.))
            .flex()
            .items_center()
            .rounded(px(4.))
            .border_1()
            .text_size(px(12.))
            .when(primary, |d| d.bg(rgb(ui().primary)).border_color(rgb(ui().primary)).text_color(rgb(ui().on_accent)))
            .when(!primary, |d| d.bg(rgb(ui().btn_bg)).border_color(rgb(ui().btn_border)).text_color(rgb(ui().text)))
            .when(disabled, |d| d.opacity(0.5))
            .when(!disabled, |d| {
                d.cursor_pointer().hover(move |h| h.bg(rgb(if primary { ui().accent_hover } else { ui().btn_hover })))
            })
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| on_click(this, window, cx)))
            .child(label.to_owned())
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let branch = self.branch.clone().unwrap_or_else(|| "(no branch)".into());
        div()
            .h(px(38.))
            .flex_none()
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(6.))
            .bg(rgb(ui().toolbar_bg))
            .border_b_1()
            .border_color(rgb(ui().divider))
            .child(self.tool_button("tb-commit", "Commit", true, cx, |this, window, cx| this.focus_commit_box(window, cx)))
            .child(div().w(px(6.)))
            .child(self.tool_button("tb-pull", "↓ Pull", false, cx, |this, _, cx| this.do_pull(cx)))
            .child(self.tool_button("tb-push", "↑ Push", false, cx, |this, _, cx| this.do_push(cx)))
            .child(self.tool_button("tb-fetch", "⟳ Fetch", false, cx, |this, _, cx| this.do_fetch(cx)))
            .child(div().w(px(6.)))
            .child(self.tool_button("tb-branch", "⎇ Branch", false, cx, |this, window, cx| this.open_new_branch(None, window, cx)))
            .child(self.tool_button("tb-stash", "Stash", false, cx, |this, _, cx| this.do_stash(cx)))
            .child(self.tool_button("tb-pop", "Pop Stash", false, cx, |this, _, cx| this.do_stash_pop(cx)))
            .child(div().flex_1())
            .children(self.identity.clone().map(|i| {
                div().text_size(px(11.)).text_color(rgb(ui().muted)).child(format!("Committing as {i}"))
            }))
            .child(self.tool_button("tb-wrap", if self.wrap { "Wrap: on" } else { "Wrap: off" }, false, cx, |this, _, cx| this.toggle_wrap(cx)))
            .when_some(self.busy.clone(), |d, label| {
                d.child(div().text_size(px(12.)).italic().text_color(rgb(ui().muted)).child(format!("{label}…")))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(branch))
                    .when(self.ahead > 0, |d| d.child(div().text_color(rgb(ui().ok_text)).child(format!("↑{}", self.ahead))))
                    .when(self.behind > 0, |d| d.child(div().text_color(rgb(ui().danger)).child(format!("↓{}", self.behind)))),
            )
    }

    fn render_notice(&self, cx: &mut Context<Self>) -> Option<gpui::Stateful<gpui::Div>> {
        let n = self.notice.as_ref()?;
        Some(
            div()
                .id("notice")
                .flex_none()
                .px(px(12.))
                .py(px(6.))
                .flex()
                .items_start()
                .gap(px(10.))
                .bg(rgb(if n.error { ui().err_bg } else { ui().ok_bg }))
                .border_b_1()
                .border_color(rgb(ui().divider))
                .text_size(px(12.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .children(n.text.lines().enumerate().map(|(i, l)| {
                            div().when(i == 0, |d| d.font_weight(FontWeight::SEMIBOLD)).child(l.to_owned())
                        })),
                )
                .child(
                    div()
                        .id("notice-close")
                        .px(px(6.))
                        .rounded(px(3.))
                        .cursor_pointer()
                        .hover(|h| h.bg(rgb(ui().notice_close_hover)))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.notice = None;
                            cx.notify();
                        }))
                        .child("×"),
                ),
        )
    }

    fn render_commit_box(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let staged = self.staged_files;
        let unstaged = self.unstaged_files;
        div()
            .flex_none()
            .p(px(10.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .bg(rgb(ui().commitbox_bg))
            .border_b_1()
            .border_color(rgb(ui().divider))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_size(px(11.)).text_color(rgb(ui().muted)).child("COMMIT MESSAGE"))
                    .child(
                        div()
                            .id("amend")
                            .text_size(px(12.))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_amend(cx)))
                            .child(if self.amend { "☑ Amend last commit" } else { "☐ Amend last commit" }),
                    ),
            )
            .child(
                div()
                    .key_context("CommitMessage")
                    .h(px(84.))
                    .bg(rgb(ui().editor_bg))
                    .rounded(px(4.))
                    .overflow_hidden()
                    .border_1()
                    .border_color(rgb(ui().input_border))
                    .on_action(cx.listener(|this, _: &DoCommit, window, cx| this.do_commit(window, cx)))
                    .child(self.message_box.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(self.tool_button(
                        "do-commit",
                        if self.amend { "Amend Commit" } else { "Commit" },
                        true,
                        cx,
                        |this, window, cx| this.do_commit(window, cx),
                    ))
                    .child(div().text_size(px(11.)).text_color(rgb(ui().muted)).child("⌘↵"))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(ui().muted))
                            .child(format!("{staged} staged · {unstaged} unstaged")),
                    ),
            )
    }

    fn render_locations(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.branch.clone();
        let sections: [(&'static str, &Vec<String>, bool); 6] = [
            ("BRANCHES", &self.refs.branches, true),
            ("REMOTES", &self.refs.remotes, true),
            ("TAGS", &self.refs.tags, true),
            ("STASHES", &self.refs.stashes, false),
            ("SUBMODULES", &self.refs.submodules, false),
            ("WORKTREES", &self.refs.worktrees, false),
        ];
        let mut col = div()
            .id("locations")
            .w(px(self.locations_w))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .bg(rgb(ui().loc_bg))
            .pt(px(8.))
            .text_size(px(12.));
        for (title, items, jumpable) in sections {
            let open = !self.collapsed.contains(title) && !items.is_empty();
            col = col.child(
                div()
                    .id(title)
                    .h(px(20.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .text_size(px(11.))
                    .text_color(rgb(ui().text))
                    .font_weight(FontWeight::SEMIBOLD)
                    .hover(|h| h.bg(rgb(ui().hover)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if !this.collapsed.remove(title) {
                            this.collapsed.insert(title);
                        }
                        cx.notify();
                    }))
                    .child(
                        svg()
                            .path(if open { "icons/chevron-down.svg" } else { "icons/chevron-right.svg" })
                            .size(px(10.))
                            .text_color(rgb(ui().muted)),
                    )
                    .child(format!("{title} ({})", items.len())),
            );
            if open {
                for (i, name) in items.iter().enumerate() {
                    let is_current = title == "BRANCHES" && current.as_deref() == Some(name.as_str());
                    let target = name.clone();
                    let switchable = title == "BRANCHES" || title == "REMOTES";
                    col = col.child(
                        div()
                            .id((title, i))
                            .h(px(20.))
                            .pl(px(26.))
                            .pr(px(8.))
                            .flex()
                            .items_center()
                            .text_color(rgb(ui().text))
                            .when(is_current, |d| d.font_weight(FontWeight::BOLD))
                            .when(jumpable, |d| {
                                d.hover(|h| h.bg(rgb(ui().hover))).on_click(cx.listener(
                                    move |this, e: &ClickEvent, _, cx| {
                                        // Click finds the commit; double-click switches to the branch.
                                        if e.click_count() >= 2 && switchable {
                                            this.switch_to(&target, cx);
                                        } else {
                                            this.jump_to_ref(&target, cx);
                                        }
                                    },
                                ))
                            })
                            .child(div().truncate().child(name.clone())),
                    );
                }
            }
        }
        col
    }

    fn render_commit_row(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let wt = self.has_wt() as usize;
        let selected = ix == self.selected;
        let base = div()
            .id(("commit", ix))
            .h(px(ROW_H))
            .w_full()
            .flex_none()
            .flex()
            .when(selected, |d| d.bg(rgb(ui().select)))
            .when(!selected, |d| d.hover(|h| h.bg(rgb(ui().hover))))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                window.focus(&this.focus_handle);
                this.select(ix, false, cx);
            }));

        if wt == 1 && ix == 0 {
            return base
                .child(
                    div().w(px(LANE_W * self.lanes as f32 + 10.)).h_full().flex_none().flex().items_center().pl(px(4.)).child(
                        div().w(px(8.)).h(px(8.)).rounded(px(1.)).bg(rgb(ui().local_bg)),
                    ),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(format!(
                            "{} uncommitted change{}",
                            self.dirty_files,
                            if self.dirty_files == 1 { "" } else { "s" }
                        )))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(ui().muted))
                                .child(format!("Working copy · {} staged", self.staged_files)),
                        ),
                );
        }

        let entry = &self.log[ix - wt];
        let commit = &entry.commit;
        let badges: Vec<RefLabel> = commit.labels.iter().take(3).cloned().collect();
        let extra = commit.labels.len().saturating_sub(3);
        base.child(graph_cell(entry.graph.clone(), self.lanes)).child(
            div()
                .flex_1()
                .min_w_0()
                .pr(px(8.))
                .flex()
                .flex_col()
                .justify_center()
                .child(div().truncate().font_weight(FontWeight::SEMIBOLD).child(commit.summary.clone()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .text_size(px(11.))
                        .text_color(rgb(ui().muted))
                        .child(div().truncate().max_w(px(120.)).flex_none().child(commit.author_name.clone()))
                        .children(badges.into_iter().map(badge))
                        .when(extra > 0, |d| d.child(format!("+{extra}")))
                        .child(div().flex_1())
                        .child(div().flex_none().child(fmt_list_date(commit.time))),
                ),
        )
    }

    fn small_button(
        &self,
        id: (&'static str, usize),
        label: &'static str,
        action: GitAction,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let danger = label == "Discard" || label == "Revert";
        div()
            .id(id)
            .h(px(15.))
            .px(px(6.))
            .flex()
            .flex_none()
            .items_center()
            .rounded(px(3.))
            .border_1()
            .border_color(rgb(if danger { ui().danger } else { ui().sbtn_border }))
            .bg(rgb(ui().sbtn_bg))
            .text_size(px(10.5))
            .font_family(".SystemUIFont")
            .text_color(rgb(if danger { ui().danger } else { ui().text }))
            .cursor_pointer()
            .hover(|h| h.bg(rgb(ui().sbtn_hover)))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                this.run_action(action.clone(), window, cx);
            }))
            .child(label)
    }

    fn render_detail_row(&self, row: &DetailRow, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let base = div().w_full().h(px(DETAIL_ROW_H)).flex_none().flex().items_center().overflow_hidden();
        match row {
            DetailRow::Meta { label, value, mono, badges } => base
                .child(
                    div()
                        .w(px(100.))
                        .flex_none()
                        .pr(px(10.))
                        .text_right()
                        .italic()
                        .text_size(px(11.))
                        .text_color(rgb(ui().muted))
                        .child(*label),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .when(*mono, |d| d.font_family(fonts::editor_family()).text_size(px(11.)))
                        .child(value.clone())
                        .children(badges.iter().cloned().map(badge)),
                )
                .into_any_element(),
            DetailRow::Message(text) => base.pl(px(100.)).child(text.clone()).into_any_element(),
            DetailRow::Gap => base.into_any_element(),
            DetailRow::Title { text, buttons } => base
                .px(px(10.))
                .gap(px(8.))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(ui().text))
                .child(text.clone())
                .children(buttons.iter().cloned().enumerate().map(|(k, (label, action))| {
                    self.small_button(("title-btn", ix * 8 + k), label, action, cx)
                }))
                .into_any_element(),
            DetailRow::Actions(buttons) => base
                .pl(px(10.))
                .gap(px(6.))
                .children(buttons.iter().cloned().enumerate().map(|(k, (label, action))| {
                    self.small_button(("action-btn", ix * 8 + k), label, action, cx)
                }))
                .into_any_element(),
            DetailRow::Notice(text) => base.px(px(10.)).italic().text_color(rgb(ui().muted)).child(text.clone()).into_any_element(),
            DetailRow::File { path, status, adds, dels, openable, buttons } => {
                let open_path = PathBuf::from(path);
                let openable = *openable;
                let label = match status {
                    FileStatus::Added => " (new)",
                    FileStatus::Deleted => " (deleted)",
                    FileStatus::Renamed => " (renamed)",
                    _ => "",
                };
                base.id(("file", ix))
                    .bg(rgb(ui().file_hdr))
                    .px(px(8.))
                    .gap(px(8.))
                    .font_family(fonts::editor_family())
                    .text_size(px(11.))
                    .when(openable, |d| {
                        d.cursor_pointer().on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                            cx.emit(GitEvent::OpenFile(open_path.clone()))
                        }))
                    })
                    .child(div().truncate().child(format!("{path}{label}")))
                    .child(stat_pill(format!("−{dels}"), ui().stat_del))
                    .child(stat_pill(format!("+{adds}"), ui().stat_add))
                    .child(div().flex_1())
                    .children(buttons.iter().cloned().enumerate().map(|(k, (label, action))| {
                        self.small_button(("file-btn", ix * 8 + k), label, action, cx)
                    }))
                    .into_any_element()
            }
            DetailRow::Hunk(header) => base
                .bg(rgb(ui().hunk_hdr))
                .px(px(10.))
                .font_family(fonts::editor_family())
                .text_size(px(10.5))
                .text_color(rgb(ui().muted))
                .child(header.clone())
                .into_any_element(),
            DetailRow::Diff(r) => base
                .font_family(fonts::editor_family())
                .text_size(px(11.5))
                .child(diff_half(&r.left))
                .child(div().w(px(1.)).h_full().flex_none().bg(rgb(ui().divider)))
                .child(diff_half(&r.right))
                .into_any_element(),
        }
    }

    fn resize_handle(&self, id: &'static str, pane: Pane, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .w(px(5.))
            .h_full()
            .flex_none()
            .cursor(CursorStyle::ResizeLeftRight)
            .bg(rgb(ui().divider))
            .hover(|h| h.bg(rgb(ui().primary)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _, _| this.start_resize(pane, e)),
            )
    }
}

// ---- free helpers -----------------------------------------------------------------------

fn push_files(rows: &mut Vec<DetailRow>, files: &[FileDiff], buttons: impl Fn(&FileDiff) -> Buttons) {
    for f in files {
        rows.push(DetailRow::File {
            path: f.display_path().to_owned(),
            status: f.status,
            adds: f.additions,
            dels: f.deletions,
            openable: f.status != FileStatus::Deleted,
            buttons: buttons(f),
        });
        if f.binary {
            rows.push(DetailRow::Notice("Binary file not shown".into()));
            continue;
        }
        let mut budget = MAX_DIFF_ROWS_PER_FILE;
        'hunks: for h in &f.hunks {
            rows.push(DetailRow::Hunk(h.header.clone()));
            for r in side_by_side(h) {
                if budget == 0 {
                    rows.push(DetailRow::Notice("… diff truncated (too large to show)".into()));
                    break 'hunks;
                }
                budget -= 1;
                rows.push(DetailRow::Diff(r));
            }
        }
        rows.push(DetailRow::Gap);
    }
}

fn local(secs: i64) -> Option<chrono::DateTime<Local>> {
    Local.timestamp_opt(secs, 0).single()
}

fn fmt_list_date(secs: i64) -> String {
    let Some(d) = local(secs) else { return String::new() };
    if d.year() == Local::now().year() {
        d.format("%a, %b %-d, %-I %p").to_string()
    } else {
        d.format("%a, %b %-d, %Y, %-I %p").to_string()
    }
}

fn fmt_detail_date(secs: i64) -> String {
    local(secs).map(|d| d.format("%a, %b %-d, %Y, %-I:%M %p").to_string()).unwrap_or_default()
}

fn badge(l: RefLabel) -> gpui::Div {
    let (bg, fg) = match l.kind {
        RefKind::Tag => (ui().tag_bg, ui().tag_text),
        RefKind::LocalBranch => (ui().local_bg, ui().on_accent),
        RefKind::RemoteBranch => (ui().remote_bg, ui().remote_text),
    };
    div()
        .h(px(15.))
        .px(px(5.))
        .flex()
        .flex_none()
        .items_center()
        .rounded(px(3.))
        .text_size(px(10.))
        .bg(rgb(bg))
        .text_color(rgb(fg))
        .when(l.is_head, |d| d.font_weight(FontWeight::BOLD))
        .max_w(px(150.))
        .child(div().truncate().child(SharedString::from(l.name)))
}

fn stat_pill(text: String, bg: u32) -> gpui::Div {
    div().px(px(4.)).rounded(px(3.)).text_size(px(10.)).bg(rgb(bg)).text_color(rgb(ui().text)).child(text)
}

/// Gutter (line number) width inside one diff half, matching `diff_half`.
const DIFF_GUTTER: f32 = 44.0;
/// Width of a monospace character at the diff's font size, as a fraction of the size (slightly
/// generous, so a wrapped line never runs under the edge).
const DIFF_CHAR_RATIO: f32 = 0.62;
const DIFF_FONT_SIZE: f32 = 11.5;

/// Characters that fit in one half of a side-by-side diff `detail_w` pixels wide.
fn diff_cols(detail_w: f32) -> usize {
    let half = (detail_w - 1.0) / 2.0 - DIFF_GUTTER - 6.0;
    (half / (DIFF_FONT_SIZE * DIFF_CHAR_RATIO)).floor().max(0.0) as usize
}

/// Expand tabs to 4 spaces, remapping the changed range to the new character positions.
fn expand_cell(cell: &Cell) -> Cell {
    let mut text = String::new();
    let mut map = Vec::with_capacity(cell.text.chars().count() + 1);
    for ch in cell.text.chars() {
        map.push(text.chars().count());
        if ch == '\t' {
            text.push_str("    ");
        } else {
            text.push(ch);
        }
    }
    map.push(text.chars().count());
    let changed = cell.changed.as_ref().map(|r| {
        let at = |i: usize| map.get(i).copied().unwrap_or(*map.last().unwrap_or(&0));
        at(r.start)..at(r.end)
    });
    Cell { lineno: cell.lineno, text, kind: cell.kind, changed }
}

/// Split one side into pieces of at most `cols` characters. Only the first piece carries the
/// line number; each piece keeps the part of the highlighted range that falls inside it.
fn wrap_cell(cell: &Cell, cols: usize) -> Vec<Cell> {
    let cell = expand_cell(cell);
    let chars: Vec<char> = cell.text.chars().collect();
    if chars.len() <= cols {
        return vec![cell];
    }
    chars
        .chunks(cols)
        .enumerate()
        .map(|(k, chunk)| {
            let (lo, hi) = (k * cols, k * cols + chunk.len());
            let changed = cell.changed.as_ref().and_then(|r| {
                let (a, b) = (r.start.max(lo), r.end.min(hi));
                (a < b).then(|| a - lo..b - lo)
            });
            Cell { lineno: if k == 0 { cell.lineno } else { 0 }, text: chunk.iter().collect(), kind: cell.kind, changed }
        })
        .collect()
}

/// A side-by-side row as one or more rows no wider than `cols` characters per side.
fn wrap_diff_row(row: &SideBySideRow, cols: usize) -> Vec<SideBySideRow> {
    let left = row.left.as_ref().map(|c| wrap_cell(c, cols)).unwrap_or_default();
    let right = row.right.as_ref().map(|c| wrap_cell(c, cols)).unwrap_or_default();
    let n = left.len().max(right.len()).max(1);
    (0..n)
        .map(|i| SideBySideRow { left: left.get(i).cloned(), right: right.get(i).cloned() })
        .collect()
}

fn wrap_detail(rows: &[DetailRow], cols: usize) -> Vec<DetailRow> {
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        match r {
            DetailRow::Diff(d) => out.extend(wrap_diff_row(d, cols).into_iter().map(DetailRow::Diff)),
            other => out.push(other.clone()),
        }
    }
    out
}

fn expand_tabs(s: &str) -> String {
    s.replace('\t', "    ")
}

/// One side of a side-by-side row.
fn diff_half(cell: &Option<Cell>) -> gpui::Div {
    let half = div().flex_1().min_w_0().h_full().flex().overflow_hidden();
    let Some(cell) = cell else { return half.bg(rgb(ui().empty_cell)) };
    let (bg, strong) = match cell.kind {
        LineKind::Removed => (ui().removed, ui().removed_strong),
        LineKind::Added => (ui().added, ui().added_strong),
        LineKind::Context => (ui().context_cell, ui().context_cell),
    };
    let mut code = div().flex().flex_row().items_center().whitespace_nowrap();
    match &cell.changed {
        Some(r) if r.start < r.end => {
            let chars: Vec<char> = cell.text.chars().collect();
            let (a, b) = (r.start.min(chars.len()), r.end.min(chars.len()));
            let piece = |range: std::ops::Range<usize>| expand_tabs(&chars[range].iter().collect::<String>());
            code = code
                .child(piece(0..a))
                .child(div().bg(rgb(strong)).child(piece(a..b)))
                .child(piece(b..chars.len()));
        }
        _ => code = code.child(expand_tabs(&cell.text)),
    }
    half.bg(rgb(bg))
        .child(
            div()
                .w(px(44.))
                .flex_none()
                .pr(px(6.))
                .flex()
                .items_center()
                .justify_end()
                .text_size(px(10.5))
                .text_color(rgb(ui().muted))
                .child(if cell.lineno == 0 { String::new() } else { cell.lineno.to_string() }),
        )
        .child(code)
}

/// The colored rail for one commit row.
fn graph_cell(row: GraphRow, lanes: usize) -> impl IntoElement {
    let width = LANE_W * lanes as f32 + 10.0;
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, _, window: &mut Window, _cx: &mut App| {
            let lane_x = |lane: usize| bounds.left() + px(8.0 + lane.min(MAX_LANES - 1) as f32 * LANE_W);
            let top = bounds.top();
            let bottom = bounds.bottom();
            let mid = top + px(f32::from(bottom - top) / 2.0);
            let color = |lane: usize| rgba(ui().lanes[lane % ui().lanes.len()]);

            let stroke = |window: &mut Window, from: gpui::Point<Pixels>, to: gpui::Point<Pixels>, ctrl: Option<gpui::Point<Pixels>>, col: Hsla| {
                let mut pb = PathBuilder::stroke(px(1.6));
                pb.move_to(from);
                match ctrl {
                    Some(ct) => pb.curve_to(to, ct),
                    None => pb.line_to(to),
                }
                if let Ok(path) = pb.build() {
                    window.paint_path(path, col);
                }
            };

            // From the top edge: pass-through lanes and lanes ending at this commit.
            for &l in &row.incoming {
                if row.converging.contains(&l) {
                    let (a, b) = (point(lane_x(l), top), point(lane_x(row.column), mid));
                    let ctrl = (l != row.column).then(|| point(lane_x(l), mid));
                    stroke(window, a, b, ctrl, color(l));
                }
            }
            // To the bottom edge: pass-throughs and links to parents.
            for e in &row.outgoing {
                let (a, b) = (point(lane_x(e.from), mid), point(lane_x(e.to), bottom));
                if e.from == e.to && e.from != row.column {
                    // Pass-through lane: runs the full height.
                    stroke(window, point(lane_x(e.from), top), b, None, color(e.to));
                } else {
                    let ctrl = (e.from != e.to).then(|| point(lane_x(e.from), bottom));
                    stroke(window, a, b, ctrl, color(e.to));
                }
            }
            let r = px(4.5);
            let center = point(lane_x(row.column), mid);
            window.paint_quad(quad(
                Bounds::new(point(center.x - r, center.y - r), size(r * 2.0, r * 2.0)),
                r,
                color(row.column),
                px(1.5),
                rgba(ui().list_bg),
                BorderStyle::Solid,
            ));
        },
    )
    .w(px(width))
    .h_full()
    .flex_none()
}

impl Render for GitView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // How many characters fit in one half of the diff, from the window and column widths.
        let detail_w = f32::from(window.viewport_size().width) - self.locations_w - self.commits_w - 10.0;
        let cols = diff_cols(detail_w);
        if cols != self.wrap_cols {
            self.wrap_cols = cols;
            self.rewrap();
        }
        let root = div()
            .id("git-view")
            .key_context("GitView")
            .track_focus(&self.focus_handle)
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(ui().list_bg))
            .text_size(px(12.))
            .text_color(rgb(ui().text))
            .on_action(cx.listener(|_, _: &CloseGit, _, cx| cx.emit(GitEvent::Close)))
            .on_action(cx.listener(|this, _: &PrevCommit, _, cx| this.move_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &NextCommit, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &PagePrevCommit, _, cx| this.move_selection(-12, cx)))
            .on_action(cx.listener(|this, _: &PageNextCommit, _, cx| this.move_selection(12, cx)))
            .on_action(cx.listener(|this, _: &FirstCommit, _, cx| this.select(0, true, cx)))
            .on_action(cx.listener(|this, _: &LastCommit, _, cx| {
                let last = this.list_len().saturating_sub(1);
                this.select(last, true, cx)
            }))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up));

        if let Some(err) = &self.error {
            return root
                .items_center()
                .justify_center()
                .text_color(rgb(ui().muted))
                .child(format!("Not a git repository ({err})"));
        }

        let commits = uniform_list(
            "commits",
            self.list_len(),
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                range.map(|ix| this.render_commit_row(ix, cx)).collect::<Vec<_>>()
            }),
        )
        .track_scroll(self.commits_scroll.clone())
        .size_full();

        let detail = uniform_list(
            "detail",
            self.shown.len(),
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|ix| {
                        let row = this.shown[ix].clone();
                        this.render_detail_row(&row, ix, cx)
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(self.detail_scroll.clone())
        .size_full();

        let working_copy = self.working_copy_selected();
        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(self.render_locations(cx))
            .child(self.resize_handle("resize-locations", Pane::Locations, cx))
            .child(
                div().w(px(self.commits_w)).h_full().flex_none().bg(rgb(ui().list_bg)).child(
                    if self.loading && self.log.is_empty() {
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .italic()
                            .text_color(rgb(ui().muted))
                            .child("Loading history…")
                            .into_any_element()
                    } else {
                        commits.into_any_element()
                    },
                ),
            )
            .child(self.resize_handle("resize-commits", Pane::Commits, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .bg(rgb(ui().detail_bg))
                    .when(working_copy, |d| d.child(self.render_commit_box(cx)))
                    .child(div().flex_1().min_h_0().child(detail)),
            );

        root.child(self.render_toolbar(cx))
            .children(self.render_notice(cx))
            .child(body)
            .children(self.prompt.as_ref().map(|(p, _)| {
                // Prompts float over the window; clicking outside cancels.
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .pt(px(46.))
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.prompt = None;
                            window.focus(&this.focus_handle);
                            cx.notify();
                        }),
                    )
                    .child(div().child(p.clone()))
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mide_git::diff::{DiffLine, Hunk};

    fn cell(n: u32, text: &str, kind: LineKind, changed: Option<std::ops::Range<usize>>) -> Cell {
        Cell { lineno: n, text: text.to_owned(), kind, changed }
    }

    #[test]
    fn short_lines_are_not_wrapped() {
        let r = SideBySideRow { left: Some(cell(1, "abc", LineKind::Context, None)), right: Some(cell(1, "abc", LineKind::Context, None)) };
        assert_eq!(wrap_diff_row(&r, 10).len(), 1);
    }

    #[test]
    fn long_lines_wrap_with_number_only_on_the_first_piece_and_highlight_split() {
        let r = SideBySideRow {
            left: Some(cell(7, "0123456789ABCDEF", LineKind::Removed, Some(8..14))),
            right: Some(cell(7, "short", LineKind::Added, None)),
        };
        let rows = wrap_diff_row(&r, 10);
        assert_eq!(rows.len(), 2);
        let l: Vec<_> = rows.iter().map(|r| r.left.clone().unwrap()).collect();
        assert_eq!((l[0].lineno, l[0].text.as_str(), l[0].changed.clone()), (7, "0123456789", Some(8..10)));
        assert_eq!((l[1].lineno, l[1].text.as_str(), l[1].changed.clone()), (0, "ABCDEF", Some(0..4)));
        // The shorter side has nothing on the continuation row.
        assert!(rows[0].right.is_some() && rows[1].right.is_none());
    }

    #[test]
    fn tabs_expand_before_wrapping_and_keep_highlights_aligned() {
        let c = expand_cell(&cell(1, "\tab", LineKind::Added, Some(1..3)));
        assert_eq!(c.text, "    ab");
        assert_eq!(c.changed, Some(4..6));
    }

    #[test]
    fn unpaired_side_stays_empty_and_cols_follow_width() {
        let r = SideBySideRow { left: None, right: Some(cell(1, &"x".repeat(25), LineKind::Added, None)) };
        let rows = wrap_diff_row(&r, 10);
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r.left.is_none()));
        assert!(diff_cols(1200.0) > diff_cols(800.0));
        assert_eq!(diff_cols(10.0), 0);
    }

    fn file(lines: Vec<DiffLine>, binary: bool, status: FileStatus) -> FileDiff {
        FileDiff {
            old_path: Some("a.txt".into()),
            new_path: Some("a.txt".into()),
            status,
            binary,
            additions: 1,
            deletions: 1,
            hunks: vec![Hunk { header: "@@ -1 +1 @@".into(), old_start: 1, new_start: 1, lines }],
        }
    }

    fn line(kind: LineKind, o: Option<u32>, n: Option<u32>, t: &str) -> DiffLine {
        DiffLine { kind, old_lineno: o, new_lineno: n, text: t.into() }
    }

    #[test]
    fn files_flatten_into_header_hunk_and_paired_rows() {
        let f = file(
            vec![line(LineKind::Removed, Some(1), None, "old"), line(LineKind::Added, None, Some(1), "new")],
            false,
            FileStatus::Modified,
        );
        let mut rows = Vec::new();
        push_files(&mut rows, &[f], |_| Vec::new());
        assert!(matches!(&rows[0], DetailRow::File { path, openable: true, .. } if path == "a.txt"));
        assert!(matches!(&rows[1], DetailRow::Hunk(h) if h == "@@ -1 +1 @@"));
        assert!(matches!(&rows[2], DetailRow::Diff(r) if r.left.is_some() && r.right.is_some()));
        assert!(matches!(rows.last(), Some(DetailRow::Gap)));
    }

    #[test]
    fn binary_and_deleted_files_are_handled() {
        let mut rows = Vec::new();
        push_files(&mut rows, &[file(vec![], true, FileStatus::Deleted)], |_| Vec::new());
        assert!(matches!(&rows[0], DetailRow::File { openable: false, .. }));
        assert!(matches!(&rows[1], DetailRow::Notice(t) if t.contains("Binary")));
    }

    #[test]
    fn huge_diffs_are_truncated() {
        let lines: Vec<DiffLine> =
            (0..MAX_DIFF_ROWS_PER_FILE as u32 + 50).map(|i| line(LineKind::Context, Some(i), Some(i), "x")).collect();
        let mut rows = Vec::new();
        push_files(&mut rows, &[file(lines, false, FileStatus::Modified)], |_| Vec::new());
        let diffs = rows.iter().filter(|r| matches!(r, DetailRow::Diff(_))).count();
        assert_eq!(diffs, MAX_DIFF_ROWS_PER_FILE);
        assert!(rows.iter().any(|r| matches!(r, DetailRow::Notice(t) if t.contains("truncated"))));
    }

    #[test]
    fn dates_and_tabs_format() {
        assert_eq!(expand_tabs("\ta"), "    a");
        assert!(!fmt_list_date(1_700_000_000).is_empty());
        assert!(fmt_detail_date(1_700_000_000).contains("2023"));
    }
}
