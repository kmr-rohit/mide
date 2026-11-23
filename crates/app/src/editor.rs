//! The text editor view: GPU-painted lines, gutter, selections, carets, IME and
//! keyboard/mouse handling on top of `mide_core::Buffer`.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use gpui::{
    actions, div, fill, point, prelude::*, px, relative, size, App, Bounds, ClipboardItem, ContentMask,
    Context, CursorStyle, ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle,
    Focusable, Font, FontFeatures, FontStyle, FontWeight, GlobalElementId, Hsla, InspectorElementId,
    IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    Render, ScrollWheelEvent, ShapedLine, SharedString, Style, TextRun, UTF16Selection,
    UnderlineStyle, Window,
};
use mide_core::search::{self, Matcher, Query};
use mide_core::{Buffer, Motion, Selection};
use mide_syntax::{DocHighlighter, Rgb, Span, Syntaxes, Theme};

use crate::find_bar::{FindBar, FindEvent};
use crate::fonts;
use crate::perf;

actions!(
    editor,
    [
        MoveLeft, MoveRight, MoveUp, MoveDown,
        SelectLeft, SelectRight, SelectUp, SelectDown,
        WordLeft, WordRight, SelectWordLeft, SelectWordRight,
        LineStart, LineEnd, SelectLineStart, SelectLineEnd,
        DocStart, DocEnd, SelectDocStart, SelectDocEnd,
        PageUp, PageDown, SelectPageUp, SelectPageDown,
        Backspace, Delete, DeleteWordBackward, Newline, Tab,
        Undo, Redo, SelectAll, Copy, Cut, Paste,
        Find, FindReplace, FindNext, FindPrev, UseSelectionForFind,
        SelectLine, DuplicateLine, DeleteLine, MoveLineUp, MoveLineDown, JoinLines,
        IndentLines, OutdentLines, ToggleComment, InsertLineAfter, InsertLineBefore,
        AddNextOccurrence, SelectAllOccurrences,
    ]
);

/// Most matches highlighted at once; keeps searching responsive in huge files.
const MAX_MATCHES: usize = 10_000;
/// Background for every find match (the current one is also selected).
const FIND_MATCH_BG: u32 = 0x6b5a2a;

/// The line-comment marker for a language, as named by the syntax definitions.
pub fn comment_token(syntax: &str) -> &'static str {
    let s = syntax.to_ascii_lowercase();
    let has = |names: &[&str]| names.iter().any(|n| s.contains(n));
    // Short names must match exactly, or "Plain Text" would count as TeX.
    if has(&["python", "ruby", "shell", "bash", "yaml", "toml", "dockerfile", "makefile", "perl", "powershell", "nginx", "cmake", "gitignore", "elixir"])
        || s == "r"
        || s == "ini"
    {
        "#"
    } else if has(&["sql", "lua", "haskell", "ada"]) {
        "--"
    } else if has(&["lisp", "clojure", "scheme"]) {
        ";;"
    } else if has(&["vim"]) {
        "\""
    } else if has(&["latex", "matlab"]) || s == "tex" {
        "%"
    } else {
        "//"
    }
}

const TAB_SIZE: usize = 4;
const GUTTER_PAD_LEFT: f32 = 12.0;
const GUTTER_PAD_RIGHT: f32 = 16.0;
/// Lines kept visible above/below the caret when scrolling to reveal it.
const SCROLL_MARGIN_LINES: f32 = 2.0;

pub fn hsla(c: Rgb) -> Hsla {
    gpui::rgb(((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32).into()
}

fn font(bold: bool, italic: bool) -> Font {
    Font {
        family: fonts::editor_family(),
        features: FontFeatures::default(),
        fallbacks: None,
        weight: if bold { FontWeight::BOLD } else { FontWeight::NORMAL },
        style: if italic { FontStyle::Italic } else { FontStyle::Normal },
    }
}

// ---- tab expansion -----------------------------------------------------------

/// A line with tabs expanded to spaces, plus the byte/column mappings needed to
/// translate between buffer positions and shaped-text positions.
struct Expanded {
    text: String,
    /// Display byte offset for every original byte offset (`len + 1` entries).
    map: Vec<usize>,
    /// `(orig_byte, display_start, display_end)` per char.
    chars: Vec<(usize, usize, usize)>,
}

impl Expanded {
    fn new(line: &str) -> Self {
        let mut text = String::with_capacity(line.len() + 8);
        let mut map = vec![0; line.len() + 1];
        let mut chars = Vec::with_capacity(line.len());
        let mut col = 0;
        for (b, ch) in line.char_indices() {
            let start = text.len();
            for k in 0..ch.len_utf8() {
                map[b + k] = start;
            }
            if ch == '\t' {
                let n = TAB_SIZE - col % TAB_SIZE;
                text.extend(std::iter::repeat_n(' ', n));
                col += n;
            } else {
                text.push(ch);
                col += 1;
            }
            chars.push((b, start, text.len()));
        }
        map[line.len()] = text.len();
        Self { text, map, chars }
    }

    fn display_of(&self, orig_byte: usize) -> usize {
        self.map[orig_byte.min(self.map.len() - 1)]
    }

    /// Original byte offset nearest to a display byte offset.
    fn orig_for_display(&self, d: usize, line_len: usize) -> usize {
        let idx = self.chars.partition_point(|c| c.1 <= d);
        let next = |i: usize| self.chars.get(i).map_or(line_len, |c| c.0);
        let Some(&(orig, start, end)) = idx.checked_sub(1).and_then(|i| self.chars.get(i)) else {
            return 0;
        };
        if d >= end || (d - start) * 2 >= (end - start) {
            next(idx)
        } else {
            orig
        }
    }
}

fn byte_of_col(line: &str, col: usize) -> usize {
    line.char_indices().nth(col).map_or(line.len(), |(b, _)| b)
}

// ---- view state ----------------------------------------------------------------

struct Row {
    shaped: ShapedLine,
    exp: Expanded,
    line: String,
}

struct Layout {
    bounds: Bounds<Pixels>,
    /// Screen x where column 0 of the text starts (already includes -scroll_x).
    text_left: Pixels,
    first_line: usize,
    rows: Vec<Row>,
}

/// What a file looked like when we last read or wrote it; a different value later
/// means something else changed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskSig {
    mtime: Option<SystemTime>,
    len: u64,
}

pub fn disk_sig(path: &Path) -> Option<DiskSig> {
    let m = std::fs::metadata(path).ok()?;
    Some(DiskSig { mtime: m.modified().ok(), len: m.len() })
}

/// What `check_disk` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskCheck {
    Unchanged,
    /// The buffer was clean, so it silently picked up the new text.
    Reloaded,
    /// The buffer has unsaved edits and the file changed too: the user must choose.
    Conflict,
    /// The file is gone.
    Deleted,
}

pub struct EditorView {
    pub buffer: Buffer,
    pub path: Option<PathBuf>,
    syntaxes: Arc<Syntaxes>,
    disk: Option<DiskSig>,
    /// The file changed on disk while this buffer has unsaved edits.
    pub disk_conflict: bool,
    pub syntax_name: String,
    /// Line numbers on/off (off for embedded boxes like the commit message).
    pub show_gutter: bool,
    highlighter: DocHighlighter,
    line_spans: BTreeMap<usize, Arc<Vec<Span>>>,
    /// Colors for a far-away window, computed from a warm-up (see `highlight_window`).
    /// Kept apart so they are dropped when the view moves on or the text changes.
    approx_spans: BTreeMap<usize, Arc<Vec<Span>>>,
    theme: Arc<Theme>,
    focus_handle: FocusHandle,
    scroll_y: f32,
    scroll_x: f32,
    layout: Option<Layout>,
    /// IME composition range (char offsets).
    marked: Option<Range<usize>>,
    selecting: bool,
    reveal_cursor: bool,
    find_bar: Option<Entity<FindBar>>,
    /// The last query, kept so ⌘G works after the bar is closed.
    find_query: Query,
    find_matcher: Option<Matcher>,
    /// Sorted, non-overlapping char ranges highlighted while the bar is open.
    find_matches: Vec<Range<usize>>,
    /// Where the caret was when the bar opened; typing searches onward from here.
    find_origin: usize,
    /// Set when an input arrives; cleared at the end of the paint it causes.
    last_input: Option<Instant>,
}

impl EditorView {
    pub fn new(
        buffer: Buffer,
        path: Option<PathBuf>,
        syntaxes: Arc<Syntaxes>,
        theme: Arc<Theme>,
        cx: &mut Context<Self>,
    ) -> Self {
        let syntax_name = syntaxes.detect(path.as_deref(), &buffer.line(0));
        let highlighter = DocHighlighter::new(syntaxes.clone(), theme.clone(), &syntax_name);
        let disk = path.as_deref().and_then(disk_sig);
        Self {
            buffer,
            syntaxes,
            disk,
            disk_conflict: false,
            path,
            syntax_name,
            show_gutter: true,
            highlighter,
            line_spans: BTreeMap::new(),
            approx_spans: BTreeMap::new(),
            theme,
            focus_handle: cx.focus_handle(),
            scroll_y: 0.0,
            scroll_x: 0.0,
            layout: None,
            marked: None,
            selecting: false,
            reveal_cursor: false,
            find_bar: None,
            find_query: Query::default(),
            find_matcher: None,
            find_matches: Vec::new(),
            find_origin: 0,
            last_input: None,
        }
    }

    pub fn text(&self) -> String {
        self.buffer.text()
    }

    /// Replace the document with `text`, caret at the end.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.buffer = Buffer::from_text(text);
        let end = self.buffer.len_chars();
        self.buffer.set_cursor(end);
        self.line_spans.clear();
        self.approx_spans.clear();
        self.highlighter.invalidate_from(0);
        self.reveal_cursor = true;
        cx.notify();
    }

    /// Replace the whole document with nothing (used after a commit).
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.buffer = Buffer::default();
        self.line_spans.clear();
        self.approx_spans.clear();
        self.highlighter.invalidate_from(0);
        self.scroll_x = 0.0;
        self.scroll_y = 0.0;
        self.marked = None;
        cx.notify();
    }

    pub fn title(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".into())
    }

    /// 1-based (line, column) of the primary caret.
    pub fn caret_line_col(&self) -> (usize, usize) {
        let head = self.buffer.selections().last().map_or(0, |s| s.head);
        let (l, c) = self.buffer.line_col(head);
        (l + 1, c + 1)
    }

    pub fn save(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let path = self.path.clone().ok_or("This file has no name yet: use File ▸ Save As (⇧⌘S)")?;
        if self.disk_conflict {
            return Err("The file changed on disk: choose Reload or Keep Mine in the bar above first".into());
        }
        self.buffer.save(&path).map_err(|e| format!("Save failed: {e}"))?;
        self.disk = disk_sig(&path);
        cx.notify();
        Ok(())
    }

    /// Write to a new path (Save As) and make that the document's file.
    pub fn save_as(&mut self, path: PathBuf, cx: &mut Context<Self>) -> Result<(), String> {
        self.buffer.save(&path).map_err(|e| format!("Save failed: {e}"))?;
        self.set_path(path, cx);
        Ok(())
    }

    /// Point this document at another file (after Save As or a rename in the sidebar).
    pub fn set_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.syntax_name = self.syntaxes.detect(Some(&path), &self.buffer.line(0));
        self.highlighter = DocHighlighter::new(self.syntaxes.clone(), self.theme.clone(), &self.syntax_name);
        self.line_spans.clear();
        self.approx_spans.clear();
        self.disk = disk_sig(&path);
        self.disk_conflict = false;
        self.path = Some(path);
        cx.notify();
    }

    /// Compare with the file on disk and react (see `DiskCheck`).
    pub fn check_disk(&mut self, cx: &mut Context<Self>) -> DiskCheck {
        let Some(path) = self.path.clone() else { return DiskCheck::Unchanged };
        let now = disk_sig(&path);
        if now == self.disk {
            return DiskCheck::Unchanged;
        }
        let Some(now) = now else {
            self.disk = None;
            return DiskCheck::Deleted;
        };
        if self.buffer.is_dirty() {
            if !self.disk_conflict {
                self.disk_conflict = true;
                cx.notify();
            }
            return DiskCheck::Conflict;
        }
        match Buffer::open(&path) {
            Ok(fresh) => {
                self.replace_buffer(fresh);
                self.disk = Some(now);
                cx.notify();
                DiskCheck::Reloaded
            }
            // Mid-write or unreadable right now; the next change event retries.
            Err(_) => DiskCheck::Unchanged,
        }
    }

    /// Throw away unsaved edits and read the file again.
    pub fn reload_from_disk(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let path = self.path.clone().ok_or("No file to reload")?;
        let fresh = Buffer::open(&path).map_err(|e| format!("Cannot reload: {e}"))?;
        self.replace_buffer(fresh);
        self.disk = disk_sig(&path);
        self.disk_conflict = false;
        cx.notify();
        Ok(())
    }

    /// Keep the unsaved text; the next save overwrites the file on disk.
    pub fn keep_mine(&mut self, cx: &mut Context<Self>) {
        self.disk = self.path.as_deref().and_then(disk_sig);
        self.disk_conflict = false;
        cx.notify();
    }

    /// Swap in new text but stay where the caret was (by line and column).
    fn replace_buffer(&mut self, mut fresh: Buffer) {
        let (line, col) = self.buffer.line_col(self.buffer.selections().last().map_or(0, |s| s.head));
        let line = line.min(fresh.len_lines().saturating_sub(1));
        let at = fresh.offset_of(line, col.min(fresh.line_len(line)));
        fresh.set_cursor(at);
        self.buffer = fresh;
        self.line_spans.clear();
        self.approx_spans.clear();
        self.highlighter.invalidate_from(0);
        self.marked = None;
        self.find_matches.clear();
        if self.find_bar.is_some() {
            self.find_origin = at;
        }
    }

    /// Replace the text with recovered, unsaved content (the buffer shows as modified).
    pub fn restore_unsaved(&mut self, text: &str, cx: &mut Context<Self>) {
        self.buffer.select_all();
        self.buffer.insert(text);
        self.after_edit(cx);
        self.buffer.set_cursor(0);
        self.scroll_y = 0.0;
    }

    /// Caret (0-based line, col) and vertical scroll, for the saved session.
    pub fn view_state(&self) -> (usize, usize, f32) {
        let head = self.buffer.selections().last().map_or(0, |s| s.head);
        let (l, c) = self.buffer.line_col(head);
        (l, c, self.scroll_y)
    }

    /// Put the caret at (line, col) and restore the scroll offset.
    pub fn restore_view(&mut self, line: usize, col: usize, scroll_y: f32) {
        let line = line.min(self.buffer.len_lines().saturating_sub(1));
        let at = self.buffer.offset_of(line, col.min(self.buffer.line_len(line)));
        self.buffer.set_cursor(at);
        self.scroll_y = scroll_y.max(0.0);
    }

    // ---- change bookkeeping --------------------------------------------------

    fn after_edit(&mut self, cx: &mut Context<Self>) {
        let line = self.buffer.last_edit_line();
        self.highlighter.invalidate_from(line);
        let _ = self.line_spans.split_off(&line);
        self.approx_spans.clear();
        if self.find_bar.is_some() {
            self.recompute_find(false, cx); // the matches moved with the text
        }
        self.touched(cx);
    }

    fn touched(&mut self, cx: &mut Context<Self>) {
        self.reveal_cursor = true;
        self.last_input = Some(Instant::now());
        cx.notify();
    }

    // ---- find & replace ------------------------------------------------------------------

    fn select_range(&mut self, r: Range<usize>, cx: &mut Context<Self>) {
        self.marked = None;
        self.buffer.set_selections(vec![Selection::new(r.start, r.end)]);
        self.touched(cx);
    }

    fn set_find_status(&mut self, text: String, is_error: bool, cx: &mut Context<Self>) {
        if let Some(bar) = &self.find_bar {
            bar.update(cx, |b, cx| b.set_status(text, is_error, cx));
        }
    }

    fn current_match_index(&self) -> Option<usize> {
        let s = self.buffer.selections().last()?;
        let i = self.find_matches.binary_search_by_key(&s.start(), |r| r.start).ok()?;
        (self.find_matches[i].end == s.end()).then_some(i)
    }

    fn update_find_status(&mut self, cx: &mut Context<Self>) {
        if self.find_matcher.is_none() {
            return;
        }
        let n = self.find_matches.len();
        let plus = if n >= MAX_MATCHES { "+" } else { "" };
        let (text, none) = match (n, self.current_match_index()) {
            (0, _) => ("No results".to_owned(), true),
            (_, Some(i)) => (format!("{} of {n}{plus}", i + 1), false),
            _ => (format!("{n}{plus} matches"), false),
        };
        self.set_find_status(text, none, cx);
    }

    /// Re-run the search for the current query. With `jump`, select the first match at
    /// or after where the bar was opened (so typing homes in on the nearest result).
    fn recompute_find(&mut self, jump: bool, cx: &mut Context<Self>) {
        match Matcher::new(&self.find_query) {
            Err(msg) => {
                self.find_matcher = None;
                self.find_matches.clear();
                self.set_find_status(msg, true, cx);
            }
            Ok(None) => {
                self.find_matcher = None;
                self.find_matches.clear();
                self.set_find_status(String::new(), false, cx);
            }
            Ok(Some(m)) => {
                self.find_matches = search::find_all(&self.buffer, &m, MAX_MATCHES);
                if jump {
                    if let Some(r) = search::find_next(&self.buffer, &m, self.find_origin, true) {
                        self.select_range(r, cx);
                    }
                }
                self.find_matcher = Some(m);
                self.update_find_status(cx);
            }
        }
        cx.notify();
    }

    fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        // A one-line selection becomes the search text, like every editor.
        let sel = self.buffer.selected_text();
        if !sel.is_empty() && !sel.contains('\n') && sel.chars().count() <= 200 {
            self.find_query.text = sel;
        }
        self.find_origin = self.buffer.selections().last().map_or(0, |s| s.start());
        let text = self.find_query.text.clone();
        match self.find_bar.clone() {
            Some(bar) => bar.update(cx, |b, cx| {
                b.set_find_text(&text, cx);
                if replace {
                    b.show_replace(true, true, cx);
                }
            }),
            None => {
                let q = self.find_query.clone();
                let bar = cx.new(|cx| FindBar::new(&q, replace, cx));
                cx.subscribe_in(&bar, window, |this, _, ev: &FindEvent, window, cx| this.on_find_event(*ev, window, cx))
                    .detach();
                self.find_bar = Some(bar);
            }
        }
        if let Some(bar) = &self.find_bar {
            window.focus(&bar.read(cx).focus_handle(cx));
        }
        self.recompute_find(false, cx);
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_bar = None;
        self.find_matches.clear();
        self.find_matcher = None;
        window.focus(&self.focus_handle);
        cx.notify();
    }

    fn on_find_event(&mut self, ev: FindEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(bar) = &self.find_bar {
            self.find_query = bar.read(cx).query();
        }
        match ev {
            FindEvent::QueryChanged => self.recompute_find(true, cx),
            FindEvent::Next => self.find_step(true, cx),
            FindEvent::Prev => self.find_step(false, cx),
            FindEvent::ReplaceOne => self.replace_one(cx),
            FindEvent::ReplaceAll => self.replace_all(cx),
            FindEvent::Close => self.close_find(window, cx),
        }
    }

    /// Select the next (or previous) match, wrapping around. Works with the bar closed.
    fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Ok(Some(m)) = Matcher::new(&self.find_query) else { return };
        let sel = *self.buffer.selections().last().expect("never empty");
        let hit = if forward {
            search::find_next(&self.buffer, &m, sel.end(), true)
        } else {
            search::find_prev(&self.buffer, &m, sel.start(), true)
        };
        match hit {
            Some(r) => self.select_range(r, cx),
            None => self.set_find_status("No results".into(), true, cx),
        }
        self.update_find_status(cx);
    }

    fn replace_one(&mut self, cx: &mut Context<Self>) {
        let Some(template) = self.find_bar.as_ref().map(|b| b.read(cx).replacement().to_owned()) else { return };
        let Ok(Some(m)) = Matcher::new(&self.find_query) else { return };
        let sel = *self.buffer.selections().last().expect("never empty");
        // Only replace if the selection really is a match; otherwise just move to the next one.
        let on_match = !sel.is_empty()
            && search::find_next(&self.buffer, &m, sel.start(), false).is_some_and(|r| r.start == sel.start() && r.end == sel.end());
        if on_match {
            let new = m.replacement(&self.buffer.slice(sel.start(), sel.end()), &template);
            self.buffer.replace_ranges(vec![(sel.start()..sel.end(), new)]);
            self.after_edit(cx);
        }
        self.find_step(true, cx);
    }

    fn replace_all(&mut self, cx: &mut Context<Self>) {
        let Some(template) = self.find_bar.as_ref().map(|b| b.read(cx).replacement().to_owned()) else { return };
        let Ok(Some(m)) = Matcher::new(&self.find_query) else { return };
        let n = search::replace_all(&mut self.buffer, &m, &template);
        self.after_edit(cx);
        let msg = if n == 0 { "No results".to_owned() } else { format!("Replaced {n} match{}", if n == 1 { "" } else { "es" }) };
        self.set_find_status(msg, n == 0, cx);
    }

    fn use_selection_for_find(&mut self, cx: &mut Context<Self>) {
        let sel = self.buffer.selected_text();
        if sel.is_empty() || sel.contains('\n') {
            return;
        }
        self.find_query.text = sel.clone();
        if let Some(bar) = self.find_bar.clone() {
            bar.update(cx, |b, cx| b.set_find_text(&sel, cx));
            self.recompute_find(false, cx);
        }
    }

    /// Scripted typing for `MIDE_PERF_TYPING`: mostly characters, a newline now and then.
    pub fn perf_type(&mut self, i: usize, cx: &mut Context<Self>) {
        let text = if i % 40 == 39 { "\n" } else { "x" };
        self.edit(|b| b.insert(text), cx);
    }

    /// Scripted scrolling for `MIDE_PERF_TYPING`.
    pub fn perf_scroll(&mut self, lines: f32, cx: &mut Context<Self>) {
        self.scroll_y += lines * fonts::line_height();
        self.clamp_scroll();
        self.last_input = Some(Instant::now());
        cx.notify();
    }

    /// Move the caret to a 1-based line and column.
    pub fn goto(&mut self, line: usize, col: usize, cx: &mut Context<Self>) {
        self.marked = None;
        self.buffer.goto_line(line, col);
        self.touched(cx);
    }

    fn do_move(&mut self, m: Motion, extend: bool, cx: &mut Context<Self>) {
        self.marked = None;
        self.buffer.move_selections(m, extend);
        self.touched(cx);
    }

    fn page_lines(&self) -> usize {
        self.layout
            .as_ref()
            .map_or(20, |l| ((f32::from(l.bounds.size.height) / fonts::line_height()) as usize).saturating_sub(1).max(1))
    }

    fn edit(&mut self, f: impl FnOnce(&mut Buffer), cx: &mut Context<Self>) {
        self.marked = None;
        f(&mut self.buffer);
        self.after_edit(cx);
    }

    /// Make sure spans for `first..first+count` are cached.
    fn ensure_spans(&mut self, first: usize, count: usize) {
        let missing: Vec<usize> = (first..first + count)
            .filter(|i| !self.line_spans.contains_key(i) && !self.approx_spans.contains_key(i))
            .collect();
        let (Some(&lo), Some(&last)) = (missing.first(), missing.last()) else { return };
        let buf = &self.buffer;
        let h = self.highlighter.highlight_window(lo, last + 1 - lo, |i| buf.line(i));
        let target = if h.approximate { &mut self.approx_spans } else { &mut self.line_spans };
        for (k, spans) in h.lines.into_iter().enumerate() {
            target.insert(lo + k, Arc::new(spans));
        }
        // Approximate colors only live while the view stays near them.
        let near = first.saturating_sub(count * 4)..first + count * 5;
        self.approx_spans.retain(|k, _| near.contains(k));
        if self.line_spans.len() > 6000 {
            self.line_spans.retain(|k, _| (first.saturating_sub(500)..first + count + 500).contains(k));
        }
    }

    fn clamp_scroll(&mut self) {
        let max_y = (self.buffer.len_lines().saturating_sub(1)) as f32 * fonts::line_height();
        self.scroll_y = self.scroll_y.clamp(0.0, max_y.max(0.0));
        self.scroll_x = self.scroll_x.max(0.0);
    }

    // ---- mouse -----------------------------------------------------------------

    fn offset_for_point(&self, pos: Point<Pixels>) -> Option<usize> {
        let l = self.layout.as_ref()?;
        let rel_y = f32::from(pos.y - l.bounds.top()) + self.scroll_y;
        let last = self.buffer.len_lines() - 1;
        let line = ((rel_y / fonts::line_height()).floor().max(0.0) as usize).min(last);
        let col = match line.checked_sub(l.first_line).and_then(|i| l.rows.get(i)) {
            Some(row) => {
                let d = row.shaped.closest_index_for_x(pos.x - l.text_left);
                let b = row.exp.orig_for_display(d, row.line.len());
                row.line[..b].chars().count()
            }
            None => 0,
        };
        Some(self.buffer.offset_of(line, col))
    }

    fn on_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        let Some(off) = self.offset_for_point(e.position) else { return };
        self.marked = None;
        self.selecting = true;
        match e.click_count {
            1 if e.modifiers.shift => self.buffer.extend_primary_to(off),
            1 if e.modifiers.platform => self.buffer.add_cursor(off),
            1 => self.buffer.set_cursor(off),
            2 => self.buffer.select_word_at(off),
            _ => self.buffer.select_line_at(off),
        }
        self.touched(cx);
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn on_mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        if let Some(off) = self.offset_for_point(e.position) {
            self.buffer.extend_primary_to(off);
            self.touched(cx);
        }
    }

    fn on_scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let d = e.delta.pixel_delta(px(fonts::line_height()));
        self.scroll_y -= f32::from(d.y);
        self.scroll_x -= f32::from(d.x);
        self.clamp_scroll();
        cx.notify();
    }

    // ---- clipboard ---------------------------------------------------------------

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.copy_text();
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Selected text, or the current line(s) when nothing is selected.
    fn copy_text(&self) -> String {
        let sel = self.buffer.selected_text();
        if !sel.is_empty() {
            return sel;
        }
        let mut lines: Vec<usize> =
            self.buffer.selections().iter().map(|s| self.buffer.line_col(s.head).0).collect();
        lines.dedup();
        lines.iter().map(|&l| format!("{}\n", self.buffer.line(l))).collect()
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.copy_text();
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        if self.buffer.selections().iter().all(|s| s.is_empty()) {
            // No selection: cut whole lines.
            let sels: Vec<Selection> = {
                let b = &self.buffer;
                b.selections()
                    .iter()
                    .map(|s| {
                        let (line, _) = b.line_col(s.head);
                        let start = b.line_start(line);
                        let end = if line + 1 < b.len_lines() { b.line_start(line + 1) } else { b.len_chars() };
                        Selection::new(start, end)
                    })
                    .collect()
            };
            self.buffer.set_selections(sels);
        }
        self.edit(|b| b.delete_selection(), cx);
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
            self.edit(|b| b.insert(&text), cx);
        }
    }

    // ---- IME helpers -----------------------------------------------------------------

    fn utf16_to_char(&self, u: usize) -> usize {
        let r = self.buffer.rope();
        r.utf16_cu_to_char(u.min(r.len_utf16_cu()))
    }

    fn char_to_utf16(&self, c: usize) -> usize {
        let r = self.buffer.rope();
        r.char_to_utf16_cu(c.min(r.len_chars()))
    }

    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.utf16_to_char(r.start)..self.utf16_to_char(r.end)
    }

    fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.char_to_utf16(r.start)..self.char_to_utf16(r.end)
    }

    /// Screen x of a buffer position, if its line is in the last layout.
    fn x_for_offset(&self, offset: usize) -> Option<(Pixels, usize)> {
        let l = self.layout.as_ref()?;
        let (line, col) = self.buffer.line_col(offset);
        let row = l.rows.get(line.checked_sub(l.first_line)?)?;
        let d = row.exp.display_of(byte_of_col(&row.line, col));
        Some((l.text_left + row.shaped.x_for_index(d), line))
    }
}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let r = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&r));
        Some(self.buffer.slice(r.start, r.end))
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let s = *self.buffer.selections().last()?;
        Some(UTF16Selection {
            range: self.range_to_utf16(&(s.start()..s.end())),
            reversed: s.head < s.anchor,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16.map(|r| self.range_from_utf16(&r)).or_else(|| self.marked.clone());
        if let Some(r) = range {
            self.buffer.set_selections(vec![Selection::new(r.start, r.end)]);
        }
        let text = text.to_owned();
        let mut chars = text.chars();
        let single = chars.next().filter(|_| chars.next().is_none());
        match single {
            // Typing one character at a time (not IME, not paste): auto-close brackets and quotes.
            Some(c) if self.show_gutter && self.marked.is_none() => {
                self.edit(|b| {
                    if !b.type_char(c) {
                        b.insert(&text);
                    }
                }, cx)
            }
            _ => self.edit(|b| b.insert(&text), cx),
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        new_selected_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|r| self.range_from_utf16(&r))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| {
                let s = self.buffer.selections().last().copied().unwrap_or(Selection::cursor(0));
                s.start()..s.end()
            });
        self.buffer.set_selections(vec![Selection::new(range.start, range.end)]);
        let owned = text.to_owned();
        self.edit(|b| b.insert(&owned), cx);
        let n = text.chars().count();
        self.marked = (n > 0).then_some(range.start..range.start + n);
        if let Some(sel) = new_selected_utf16 {
            // Selection is relative to the inserted text.
            let base = self.char_to_utf16(range.start);
            let r = self.range_from_utf16(&(base + sel.start..base + sel.end));
            self.buffer.set_selections(vec![Selection::new(r.start, r.end)]);
        }
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let r = self.range_from_utf16(&range_utf16);
        let (x0, line) = self.x_for_offset(r.start)?;
        let (x1, _) = self.x_for_offset(r.end).unwrap_or((x0, line));
        let l = self.layout.as_ref()?;
        let y = l.bounds.top() + px(line as f32 * fonts::line_height() - self.scroll_y);
        let _ = element_bounds;
        Some(Bounds::new(point(x0, y), size((x1 - x0).max(px(2.)), px(fonts::line_height()))))
    }

    fn character_index_for_point(
        &mut self,
        pos: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let off = self.offset_for_point(pos)?;
        Some(self.char_to_utf16(off))
    }
}

impl Render for EditorView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        macro_rules! mv {
            ($action:ty, $motion:expr, $extend:expr) => {
                cx.listener(|this: &mut Self, _: &$action, _: &mut Window, cx: &mut Context<Self>| {
                    this.do_move($motion, $extend, cx)
                })
            };
        }
        let editor = div()
            .id("editor")
            .key_context("Editor")
            .track_focus(&self.focus_handle)
            .size_full()
            .cursor(CursorStyle::IBeam)
            .on_action(mv!(MoveLeft, Motion::Left, false))
            .on_action(mv!(MoveRight, Motion::Right, false))
            .on_action(mv!(MoveUp, Motion::Up(1), false))
            .on_action(mv!(MoveDown, Motion::Down(1), false))
            .on_action(mv!(SelectLeft, Motion::Left, true))
            .on_action(mv!(SelectRight, Motion::Right, true))
            .on_action(mv!(SelectUp, Motion::Up(1), true))
            .on_action(mv!(SelectDown, Motion::Down(1), true))
            .on_action(mv!(WordLeft, Motion::WordLeft, false))
            .on_action(mv!(WordRight, Motion::WordRight, false))
            .on_action(mv!(SelectWordLeft, Motion::WordLeft, true))
            .on_action(mv!(SelectWordRight, Motion::WordRight, true))
            .on_action(mv!(LineStart, Motion::LineStart, false))
            .on_action(mv!(LineEnd, Motion::LineEnd, false))
            .on_action(mv!(SelectLineStart, Motion::LineStart, true))
            .on_action(mv!(SelectLineEnd, Motion::LineEnd, true))
            .on_action(mv!(DocStart, Motion::DocStart, false))
            .on_action(mv!(DocEnd, Motion::DocEnd, false))
            .on_action(mv!(SelectDocStart, Motion::DocStart, true))
            .on_action(mv!(SelectDocEnd, Motion::DocEnd, true))
            .on_action(cx.listener(|this, _: &PageUp, _, cx| {
                let n = this.page_lines();
                this.do_move(Motion::Up(n), false, cx)
            }))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| {
                let n = this.page_lines();
                this.do_move(Motion::Down(n), false, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectPageUp, _, cx| {
                let n = this.page_lines();
                this.do_move(Motion::Up(n), true, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectPageDown, _, cx| {
                let n = this.page_lines();
                this.do_move(Motion::Down(n), true, cx)
            }))
            .on_action(cx.listener(|this, _: &Backspace, _, cx| this.edit(|b| b.delete_backward(), cx)))
            .on_action(cx.listener(|this, _: &Delete, _, cx| this.edit(|b| b.delete_forward(), cx)))
            .on_action(cx.listener(|this, _: &DeleteWordBackward, _, cx| {
                this.edit(|b| b.delete_word_backward(), cx)
            }))
            .on_action(cx.listener(|this, _: &Newline, _, cx| {
                // Python and YAML open a block after a trailing colon.
                let colon = matches!(this.syntax_name.as_str(), "Python" | "YAML");
                if this.show_gutter {
                    this.edit(|b| b.insert_newline_auto(colon), cx)
                } else {
                    this.edit(|b| b.insert_newline(), cx)
                }
            }))
            .on_action(cx.listener(|this, _: &Tab, _, cx| {
                // Tab with several lines selected indents them; otherwise it types a tab stop.
                if this.buffer.selection_spans_lines() {
                    this.edit(|b| { let n = b.indent().width(); b.indent_lines(n) }, cx)
                } else {
                    this.edit(|b| { let n = b.indent().width(); b.insert_tab(n) }, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &IndentLines, _, cx| this.edit(|b| { let n = b.indent().width(); b.indent_lines(n) }, cx)))
            .on_action(cx.listener(|this, _: &OutdentLines, _, cx| this.edit(|b| { let n = b.indent().width(); b.outdent_lines(n) }, cx)))
            .on_action(cx.listener(|this, _: &DuplicateLine, _, cx| this.edit(|b| b.duplicate_line(), cx)))
            .on_action(cx.listener(|this, _: &DeleteLine, _, cx| this.edit(|b| b.delete_line(), cx)))
            .on_action(cx.listener(|this, _: &MoveLineUp, _, cx| this.edit(|b| b.move_lines(true), cx)))
            .on_action(cx.listener(|this, _: &MoveLineDown, _, cx| this.edit(|b| b.move_lines(false), cx)))
            .on_action(cx.listener(|this, _: &JoinLines, _, cx| this.edit(|b| b.join_lines(), cx)))
            .on_action(cx.listener(|this, _: &InsertLineAfter, _, cx| this.edit(|b| b.insert_line_after(), cx)))
            .on_action(cx.listener(|this, _: &InsertLineBefore, _, cx| this.edit(|b| b.insert_line_before(), cx)))
            .on_action(cx.listener(|this, _: &ToggleComment, _, cx| {
                let token = comment_token(&this.syntax_name);
                this.edit(|b| b.toggle_comment(token), cx)
            }))
            .on_action(cx.listener(|this, _: &SelectLine, _, cx| {
                this.buffer.select_line();
                this.touched(cx);
            }))
            .on_action(cx.listener(|this, _: &AddNextOccurrence, _, cx| {
                this.buffer.add_next_occurrence();
                this.touched(cx);
            }))
            .on_action(cx.listener(|this, _: &SelectAllOccurrences, _, cx| {
                this.buffer.select_all_occurrences();
                this.touched(cx);
            }))
            .on_action(cx.listener(|this, _: &Find, window, cx| this.open_find(false, window, cx)))
            .on_action(cx.listener(|this, _: &FindReplace, window, cx| this.open_find(true, window, cx)))
            .on_action(cx.listener(|this, _: &FindNext, _, cx| this.find_step(true, cx)))
            .on_action(cx.listener(|this, _: &FindPrev, _, cx| this.find_step(false, cx)))
            .on_action(cx.listener(|this, _: &UseSelectionForFind, _, cx| this.use_selection_for_find(cx)))
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                this.marked = None;
                if this.buffer.undo() {
                    this.after_edit(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                this.marked = None;
                if this.buffer.redo() {
                    this.after_edit(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                this.buffer.select_all();
                cx.notify();
            }))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(EditorElement { editor: cx.entity() });
        // The find bar floats over the editor but is *not* inside its key context, so the
        // editor's shortcuts (arrows, Enter, ...) don't fire while typing in the bar.
        div()
            .size_full()
            .relative()
            .child(editor)
            .children(self.find_bar.clone().map(|bar| div().absolute().top(px(8.)).right(px(18.)).child(bar)))
    }
}

// ---- the painting element -------------------------------------------------------------

struct EditorElement {
    editor: Entity<EditorView>,
}

struct Frame {
    gutter_w: f32,
    cell_w: f32,
    scroll_x: f32,
    scroll_y: f32,
    first_line: usize,
    rows: Vec<Row>,
    numbers: Vec<ShapedLine>,
    current_lines: Vec<usize>,
    /// Find matches in view: (line, start col, end col).
    matches: Vec<(usize, usize, usize)>,
    focused: bool,
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl gpui::Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Frame;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Frame {
        let started = Instant::now();
        let font_size = px(fonts::editor_size());
        let base_font = font(false, false);
        let font_id = window.text_system().resolve_font(&base_font);
        let cell_w = window.text_system().em_advance(font_id, font_size).map_or(7.8, f32::from);
        let focused = self.editor.read(cx).focus_handle.is_focused(window);
        let height = f32::from(bounds.size.height);

        self.editor.update(cx, |ed, _| {
            let n_lines = ed.buffer.len_lines();
            let digits = n_lines.to_string().len().max(3);
            let gutter_w = if ed.show_gutter {
                GUTTER_PAD_LEFT + digits as f32 * cell_w + GUTTER_PAD_RIGHT
            } else {
                8.0
            };
            let text_w = f32::from(bounds.size.width) - gutter_w;

            // 1) vertical scroll: reveal the caret if an edit/motion asked for it.
            if ed.reveal_cursor {
                if let Some(head) = ed.buffer.selections().last().map(|s| s.head) {
                    let y = ed.buffer.line_col(head).0 as f32 * fonts::line_height();
                    let margin = SCROLL_MARGIN_LINES * fonts::line_height();
                    if y < ed.scroll_y + margin {
                        ed.scroll_y = y - margin;
                    } else if y + fonts::line_height() > ed.scroll_y + height - margin {
                        ed.scroll_y = y + fonts::line_height() - height + margin;
                    }
                }
            }
            ed.clamp_scroll();

            // 2) shape the visible lines.
            let first_line = (ed.scroll_y / fonts::line_height()).floor() as usize;
            let count = ((height / fonts::line_height()).ceil() as usize + 1).min(n_lines.saturating_sub(first_line));
            ed.ensure_spans(first_line, count);

            let base = ed.theme.base_style();
            let mut rows = Vec::with_capacity(count);
            let mut numbers = Vec::with_capacity(count);
            for i in first_line..first_line + count {
                let line = ed.buffer.line(i);
                let exp = Expanded::new(&line);
                let spans = ed.line_spans.get(&i).or_else(|| ed.approx_spans.get(&i)).cloned().unwrap_or_default();
                let runs = build_runs(&exp, &spans, &ed.theme, base.fg);
                let shaped = window.text_system().shape_line(
                    SharedString::from(exp.text.clone()),
                    font_size,
                    &runs,
                    None,
                );
                rows.push(Row { shaped, exp, line });
            }
            let current_lines: Vec<usize> =
                ed.buffer.selections().iter().map(|s| ed.buffer.line_col(s.head).0).collect();
            let number_range = if ed.show_gutter { first_line..first_line + count } else { 0..0 };
            for i in number_range {
                let text = (i + 1).to_string();
                let is_current = current_lines.contains(&i);
                let color = hsla(if is_current { ed.theme.foreground } else { ed.theme.gutter_foreground });
                let run = TextRun {
                    len: text.len(),
                    font: font(is_current, false),
                    color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                numbers.push(window.text_system().shape_line(text.into(), font_size, &[run], None));
            }

            // 3) horizontal scroll: reveal the caret column.
            if ed.reveal_cursor {
                if let Some(head) = ed.buffer.selections().last().map(|s| s.head) {
                    let (line, col) = ed.buffer.line_col(head);
                    if let Some(row) = line.checked_sub(first_line).and_then(|i| rows.get(i)) {
                        let x = f32::from(row.shaped.x_for_index(row.exp.display_of(byte_of_col(&row.line, col))));
                        let pad = 4.0 * cell_w;
                        if x < ed.scroll_x + pad {
                            ed.scroll_x = (x - pad).max(0.0);
                        } else if x > ed.scroll_x + text_w - pad {
                            ed.scroll_x = x - text_w + pad;
                        }
                    }
                }
                ed.reveal_cursor = false;
            }
            ed.clamp_scroll();

            let text_left = bounds.left() + px(gutter_w - ed.scroll_x);
            // Reborrow-free: store the layout used by mouse handling.
            ed.layout = Some(Layout {
                bounds,
                text_left,
                first_line,
                rows: rows.iter().map(|r| Row {
                    shaped: r.shaped.clone(),
                    exp: Expanded::new(&r.line),
                    line: r.line.clone(),
                }).collect(),
            });

            // Find matches intersecting the visible lines.
            let (lo, hi) = (ed.buffer.line_start(first_line), ed.buffer.line_start(first_line + count));
            let a = ed.find_matches.partition_point(|r| r.end <= lo);
            let b = ed.find_matches.partition_point(|r| r.start < hi);
            let matches: Vec<(usize, usize, usize)> = ed.find_matches[a..b.max(a)]
                .iter()
                .map(|r| {
                    let (line, col) = ed.buffer.line_col(r.start);
                    (line, col, col + (r.end - r.start))
                })
                .collect();

            perf::note_prepaint(started);
            Frame {
                gutter_w,
                cell_w,
                scroll_x: ed.scroll_x,
                scroll_y: ed.scroll_y,
                first_line,
                rows,
                numbers,
                current_lines,
                matches,
                focused,
            }
        })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        frame: &mut Frame,
        window: &mut Window,
        cx: &mut App,
    ) {
        let started = Instant::now();
        let focus = self.editor.read(cx).focus_handle.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.editor.clone()), cx);

        // Everything needed from the editor is copied out first, so no borrow of
        // `cx` is held while painting.
        struct SelPos {
            start: (usize, usize),
            end: (usize, usize),
            head: (usize, usize),
            empty: bool,
        }
        let (theme, sels) = {
            let ed = self.editor.read(cx);
            let b = &ed.buffer;
            let sels: Vec<SelPos> = b
                .selections()
                .iter()
                .map(|s| SelPos {
                    start: b.line_col(s.start()),
                    end: b.line_col(s.end()),
                    head: b.line_col(s.head),
                    empty: s.is_empty(),
                })
                .collect();
            (ed.theme.clone(), sels)
        };
        let last_visible = frame.first_line + frame.rows.len().saturating_sub(1);
        let is_visible = |line: usize| line >= frame.first_line && line <= last_visible && !frame.rows.is_empty();

        let top = bounds.top();
        let scroll_y = frame.scroll_y;
        let y_of = |line: usize| top + px(line as f32 * fonts::line_height() - scroll_y);
        let text_left = bounds.left() + px(frame.gutter_w - frame.scroll_x);
        let lh = px(fonts::line_height());

        // Background.
        window.paint_quad(fill(bounds, hsla(theme.background)));

        // Current-line highlight (only for caret lines without a selection).
        for s in sels.iter().filter(|s| s.empty) {
            let line = s.head.0;
            if is_visible(line) {
                window.paint_quad(fill(
                    Bounds::new(point(bounds.left(), y_of(line)), size(bounds.size.width, lh)),
                    hsla(theme.line_highlight),
                ));
            }
        }

        let text_bounds = Bounds::new(
            point(bounds.left() + px(frame.gutter_w), bounds.top()),
            size(bounds.size.width - px(frame.gutter_w), bounds.size.height),
        );
        let rows = &frame.rows;
        let first_line = frame.first_line;
        let cell_w = frame.cell_w;
        let focused = frame.focused;
        window.with_content_mask(Some(ContentMask { bounds: text_bounds }), |window| {
            // Find matches sit under the selection.
            let match_color = hsla(Rgb::hex(FIND_MATCH_BG));
            for &(line, c0, c1) in &frame.matches {
                let Some(row) = line.checked_sub(first_line).and_then(|i| rows.get(i)) else { continue };
                let x = |col: usize| row.shaped.x_for_index(row.exp.display_of(byte_of_col(&row.line, col)));
                let (x0, x1) = (x(c0), x(c1));
                window.paint_quad(fill(
                    Bounds::new(point(text_left + x0, y_of(line)), size((x1 - x0).max(px(2.)), lh)),
                    match_color,
                ));
            }

            // Selections.
            let sel_color = hsla(theme.selection);
            for s in sels.iter().filter(|s| !s.empty) {
                let ((l0, c0), (l1, c1)) = (s.start, s.end);
                for line in l0.max(first_line)..=l1.min(last_visible) {
                    let Some(row) = rows.get(line - first_line) else { continue };
                    let from_col = if line == l0 { c0 } else { 0 };
                    let (to_col, newline_pad) =
                        if line == l1 { (c1, 0.0) } else { (row.line.chars().count(), cell_w) };
                    let x = |col: usize| row.shaped.x_for_index(row.exp.display_of(byte_of_col(&row.line, col)));
                    let (x0, x1) = (x(from_col), x(to_col) + px(newline_pad));
                    window.paint_quad(fill(
                        Bounds::new(point(text_left + x0, y_of(line)), size((x1 - x0).max(px(0.)), lh)),
                        sel_color,
                    ));
                }
            }

            // Text.
            for (i, row) in rows.iter().enumerate() {
                let _ = row.shaped.paint(point(text_left, y_of(first_line + i)), lh, window, cx);
            }

            // Carets.
            if focused {
                let caret = hsla(theme.caret);
                for s in &sels {
                    let (line, col) = s.head;
                    let Some(row) = line.checked_sub(first_line).and_then(|i| rows.get(i)) else { continue };
                    let x = row.shaped.x_for_index(row.exp.display_of(byte_of_col(&row.line, col)));
                    window.paint_quad(fill(
                        Bounds::new(point(text_left + x, y_of(line)), size(px(2.), lh)),
                        caret,
                    ));
                }
            }
        });

        // Gutter (drawn last so scrolled text never shows through).
        window.paint_quad(fill(
            Bounds::new(bounds.origin, size(px(frame.gutter_w), bounds.size.height)),
            hsla(theme.background),
        ));
        for (i, num) in frame.numbers.iter().enumerate() {
            let line = frame.first_line + i;
            let x = bounds.left() + px(frame.gutter_w - GUTTER_PAD_RIGHT) - num.width;
            if frame.current_lines.contains(&line) {
                window.paint_quad(fill(
                    Bounds::new(point(bounds.left(), y_of(line)), size(px(frame.gutter_w), lh)),
                    hsla(theme.line_highlight),
                ));
            }
            let _ = num.paint(point(x, y_of(line)), lh, window, cx);
        }

        perf::note_paint(started, self.editor.read(cx).last_input);
        self.editor.update(cx, |ed, _| ed.last_input = None);
    }
}

fn build_runs(exp: &Expanded, spans: &[Span], theme: &Theme, base_fg: Rgb) -> Vec<TextRun> {
    let total = exp.text.len();
    let mk = |len: usize, fg: Rgb, bold: bool, italic: bool, underline: bool| TextRun {
        len,
        font: font(bold, italic),
        color: hsla(fg),
        background_color: None,
        underline: underline.then(|| UnderlineStyle {
            color: Some(hsla(fg)),
            thickness: px(1.),
            wavy: false,
        }),
        strikethrough: None,
    };
    let _ = theme;
    let mut runs = Vec::with_capacity(spans.len() + 1);
    let mut pos = 0;
    for sp in spans {
        let (s, e) = (exp.display_of(sp.start), exp.display_of(sp.end));
        if s > pos {
            runs.push(mk(s - pos, base_fg, false, false, false));
        }
        if e > s.max(pos) {
            let st = sp.style;
            runs.push(mk(e - s.max(pos), st.fg, st.bold, st.italic, st.underline));
            pos = e;
        }
    }
    if pos < total {
        runs.push(mk(total - pos, base_fg, false, false, false));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use mide_syntax::Style;

    fn total(runs: &[TextRun]) -> usize {
        runs.iter().map(|r| r.len).sum()
    }

    #[test]
    fn tabs_expand_to_stops_and_map_back() {
        let e = Expanded::new("\tab\tc");
        assert_eq!(e.text, "    ab  c"); // tab -> 4, then 'a','b' (col 6), tab -> 2
        assert_eq!(e.display_of(0), 0);
        assert_eq!(e.display_of(1), 4); // after the first tab
        assert_eq!(e.display_of(4), 8); // after the second tab
        assert_eq!(e.display_of(5), 9); // end of line
        // Clicking inside the first tab snaps to the nearer side.
        assert_eq!(e.orig_for_display(1, 5), 0);
        assert_eq!(e.orig_for_display(3, 5), 1);
        assert_eq!(e.orig_for_display(9, 5), 5); // past the end
    }

    #[test]
    fn multibyte_chars_map_on_char_boundaries() {
        let line = "é→x"; // 2-byte, 3-byte, 1-byte
        let e = Expanded::new(line);
        assert_eq!(e.text, line);
        assert_eq!(e.display_of(0), 0);
        assert_eq!(e.display_of(2), 2);
        assert_eq!(e.display_of(5), 5);
        assert_eq!(e.orig_for_display(2, line.len()), 2);
        assert_eq!(byte_of_col(line, 2), 5);
        assert_eq!(byte_of_col(line, 99), line.len());
    }

    #[test]
    fn runs_tile_the_display_text_exactly() {
        let theme = Theme::default_dark();
        let fg = theme.foreground;
        let st = |c| Style { fg: Rgb::hex(c), bold: false, italic: false, underline: false };
        let line = "\tkey: \"v\"";
        let e = Expanded::new(line);
        let spans = vec![
            Span { start: 0, end: 1, style: st(0x111111) },
            Span { start: 1, end: 4, style: st(0x222222) },
            // Gap at 4..6 must be filled with the base color.
            Span { start: 6, end: 9, style: st(0x333333) },
        ];
        let runs = build_runs(&e, &spans, &theme, fg);
        assert_eq!(total(&runs), e.text.len());
        assert!(runs.iter().all(|r| r.len > 0));
        // No spans at all: one base run; empty line: no runs.
        assert_eq!(build_runs(&e, &[], &theme, fg).len(), 1);
        assert!(build_runs(&Expanded::new(""), &[], &theme, fg).is_empty());
    }

    #[test]
    fn runs_tile_unicode_and_trailing_spans() {
        let theme = Theme::default_dark();
        let st = Style { fg: theme.foreground, bold: true, italic: true, underline: true };
        let line = "é→x";
        let e = Expanded::new(line);
        let runs = build_runs(&e, &[Span { start: 0, end: line.len(), style: st }], &theme, theme.foreground);
        assert_eq!(total(&runs), line.len());
        assert_eq!(runs.len(), 1);
        assert!(runs[0].underline.is_some());
    }

    #[test]
    fn comment_tokens_follow_the_language() {
        assert_eq!(comment_token("Rust"), "//");
        assert_eq!(comment_token("JavaScript"), "//");
        assert_eq!(comment_token("Python"), "#");
        assert_eq!(comment_token("YAML"), "#");
        assert_eq!(comment_token("TOML"), "#");
        assert_eq!(comment_token("Bourne Again Shell (bash)"), "#");
        assert_eq!(comment_token("Dockerfile"), "#");
        assert_eq!(comment_token("SQL"), "--");
        assert_eq!(comment_token("Lua"), "--");
        assert_eq!(comment_token("Plain Text"), "//");
        assert_eq!(comment_token("LaTeX"), "%");
        assert_eq!(comment_token("Markdown"), "//");
    }
}
