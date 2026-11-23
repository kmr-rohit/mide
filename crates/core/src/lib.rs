//! UI-independent text core: rope buffer, multi-cursor selections, motions,
//! undo/redo, and file load/save.
//!
//! All offsets are *char* offsets into the rope (not bytes, not graphemes).
//! Internally text always uses `\n`; the original line ending is remembered and
//! restored on save.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use ropey::Rope;
use unicode_segmentation::GraphemeCursor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
    /// Column to return to on vertical motion (remembered across short lines).
    pub goal_col: Option<usize>,
}

impl Selection {
    pub fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head, goal_col: None }
    }
    pub fn cursor(at: usize) -> Self {
        Self::new(at, at)
    }
    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }
    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    /// Up/Down by this many lines.
    Up(usize),
    Down(usize),
    WordLeft,
    WordRight,
    /// First non-blank char of the line, then column 0 on a second press.
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    Crlf,
}

#[derive(Debug, Clone)]
struct Edit {
    at: usize,
    removed: String,
    inserted: String,
}

/// What an edit was, so consecutive typing / backspacing can share one undo step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TxKind {
    Other,
    Typing,
    Deleting,
}

/// Typing pauses longer than this start a new undo step.
const DEFAULT_GROUP_WINDOW: Duration = Duration::from_millis(1000);

#[derive(Debug, Clone)]
struct Transaction {
    id: u64,
    kind: TxKind,
    time: Instant,
    /// In application order.
    edits: Vec<Edit>,
    before: Vec<Selection>,
    after: Vec<Selection>,
}

pub struct Buffer {
    rope: Rope,
    selections: Vec<Selection>,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    revision: u64,
    next_tx: u64,
    /// Id of the undo-stack top when last saved (0 = pristine).
    save_tx: u64,
    line_ending: LineEnding,
    /// The file was not valid UTF-8 and was loaded lossily; saving is refused
    /// so we never silently corrupt it.
    lossy: bool,
    /// Char offset of the earliest change in the last edit/undo/redo.
    last_edit_at: usize,
    group_window: Duration,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::from_text("")
    }
}

impl Buffer {
    pub fn from_text(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            selections: vec![Selection::cursor(0)],
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            next_tx: 1,
            save_tx: 0,
            line_ending: LineEnding::Lf,
            lossy: false,
            last_edit_at: 0,
            group_window: DEFAULT_GROUP_WINDOW,
        }
    }

    /// How long a pause ends a run of typing/backspacing as one undo step.
    pub fn set_undo_grouping(&mut self, window: Duration) {
        self.group_window = window;
    }

    // ---- files -----------------------------------------------------------

    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        // A NUL byte in the first 8 KiB means this is not text (image, archive, executable).
        // Checked before reading the rest so a huge binary is never pulled into memory.
        {
            use std::io::Read;
            let mut head = Vec::with_capacity(8192);
            std::fs::File::open(path)?.take(8192).read_to_end(&mut head)?;
            if head.contains(&0) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "binary file"));
            }
        }
        let bytes = std::fs::read(path)?;
        let (text, lossy) = match String::from_utf8(bytes) {
            Ok(s) => (s, false),
            Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), true),
        };
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned();
        let crlf = text.matches("\r\n").count();
        let lf = text.matches('\n').count() - crlf;
        let line_ending = if crlf > lf { LineEnding::Crlf } else { LineEnding::Lf };
        let mut b = Self::from_text(&text.replace("\r\n", "\n"));
        b.line_ending = line_ending;
        b.lossy = lossy;
        Ok(b)
    }

    /// Atomically write to `path` (temp file + rename) and mark the buffer clean.
    pub fn save(&mut self, path: impl AsRef<Path>) -> io::Result<()> {
        if self.lossy {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "file is not valid UTF-8; refusing to overwrite it",
            ));
        }
        let path = path.as_ref();
        let mut text = self.rope.to_string();
        if self.line_ending == LineEnding::Crlf {
            text = text.replace('\n', "\r\n");
        }
        let tmp = path.with_file_name(format!(
            ".{}.mide-tmp",
            path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
        ));
        std::fs::write(&tmp, text)?;
        if let Ok(meta) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
        self.save_tx = self.top_tx();
        Ok(())
    }

    fn top_tx(&self) -> u64 {
        self.undo.last().map_or(0, |t| t.id)
    }

    pub fn is_dirty(&self) -> bool {
        self.top_tx() != self.save_tx
    }
    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }
    pub fn is_lossy(&self) -> bool {
        self.lossy
    }

    // ---- accessors -------------------------------------------------------

    pub fn rope(&self) -> &Rope {
        &self.rope
    }
    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }
    pub fn selections(&self) -> &[Selection] {
        &self.selections
    }
    /// Bumped on every content change; lets the UI know when to re-layout.
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Number of lines. A trailing newline does not create an extra empty line
    /// in the count the way ropey does; an empty buffer has one line.
    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    /// Line `i` without its line terminator. Out-of-range gives "".
    pub fn line(&self, i: usize) -> String {
        if i >= self.rope.len_lines() {
            return String::new();
        }
        let mut s = self.rope.line(i).to_string();
        while s.ends_with('\n') || s.ends_with('\r') {
            s.pop();
        }
        s
    }

    /// Run `f` on line `i` (without its terminator), avoiding an allocation when the
    /// line sits in one rope chunk, which is nearly always. For whole-file scans.
    pub fn with_line<R>(&self, i: usize, f: impl FnOnce(&str) -> R) -> R {
        if i >= self.rope.len_lines() {
            return f("");
        }
        let slice = self.rope.line(i);
        match slice.as_str() {
            Some(s) => f(s.trim_end_matches(['\n', '\r'])),
            None => f(slice.to_string().trim_end_matches(['\n', '\r'])),
        }
    }

    /// Chars in line `i`, excluding the terminator.
    pub fn line_len(&self, i: usize) -> usize {
        if i >= self.rope.len_lines() {
            return 0;
        }
        let l = self.rope.line(i);
        let mut n = l.len_chars();
        while n > 0 && matches!(l.char(n - 1), '\n' | '\r') {
            n -= 1;
        }
        n
    }

    pub fn line_start(&self, i: usize) -> usize {
        self.rope.line_to_char(i.min(self.rope.len_lines()))
    }

    /// (line, column) of a char offset; column counts chars.
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.len_chars());
        let line = self.rope.char_to_line(offset);
        (line, offset - self.rope.line_to_char(line))
    }

    /// Char offset for (line, column), clamping the column to the line.
    pub fn offset_of(&self, line: usize, col: usize) -> usize {
        let line = line.min(self.rope.len_lines() - 1);
        self.rope.line_to_char(line) + col.min(self.line_len(line))
    }

    pub fn slice(&self, start: usize, end: usize) -> String {
        self.rope.slice(start..end).to_string()
    }

    /// All selected text, one entry per selection joined with `\n`.
    pub fn selected_text(&self) -> String {
        self.selections
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| self.slice(s.start(), s.end()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // ---- selections ------------------------------------------------------

    pub fn set_selections(&mut self, sels: Vec<Selection>) {
        self.selections = sels;
        self.normalize();
    }

    pub fn set_cursor(&mut self, at: usize) {
        self.set_selections(vec![Selection::cursor(at)]);
    }

    /// Extend the primary (last) selection's head to `to` (mouse drag / shift-click).
    pub fn extend_primary_to(&mut self, to: usize) {
        if let Some(last) = self.selections.last_mut() {
            last.head = to.min(self.rope.len_chars());
            last.goal_col = None;
        }
        self.normalize();
    }

    /// Add a cursor (Cmd+click).
    pub fn add_cursor(&mut self, at: usize) {
        self.selections.push(Selection::cursor(at.min(self.len_chars())));
        self.normalize();
    }

    pub fn select_all(&mut self) {
        self.selections = vec![Selection::new(0, self.len_chars())];
    }

    /// Select the word (or whitespace/punctuation run) around `at`.
    pub fn select_word_at(&mut self, at: usize) {
        let (s, e) = self.word_range(at);
        self.selections = vec![Selection::new(s, e)];
    }

    pub fn select_line_at(&mut self, at: usize) {
        let (line, _) = self.line_col(at);
        let start = self.line_start(line);
        let end = if line + 1 < self.rope.len_lines() { self.line_start(line + 1) } else { self.len_chars() };
        self.selections = vec![Selection::new(start, end)];
    }

    fn word_range(&self, at: usize) -> (usize, usize) {
        let len = self.len_chars();
        let at = at.min(len);
        let class_at = |i: usize| char_class(self.rope.char(i));
        let probe = if at < len { at } else { at.saturating_sub(1) };
        if len == 0 {
            return (0, 0);
        }
        let c = class_at(probe);
        let (mut s, mut e) = (probe, probe + 1);
        while s > 0 && class_at(s - 1) == c {
            s -= 1;
        }
        while e < len && class_at(e) == c {
            e += 1;
        }
        (s, e)
    }

    /// Sort and merge overlapping / duplicate selections.
    fn normalize(&mut self) {
        let len = self.len_chars();
        for s in &mut self.selections {
            s.anchor = s.anchor.min(len);
            s.head = s.head.min(len);
        }
        self.selections.sort_by_key(|s| (s.start(), s.end()));
        let mut out: Vec<Selection> = Vec::with_capacity(self.selections.len());
        for s in self.selections.drain(..) {
            match out.last_mut() {
                Some(last)
                    if s.start() < last.end()
                        || (s.is_empty() && last.is_empty() && s.start() == last.start()) =>
                {
                    if s.end() > last.end() {
                        last.head = s.end();
                        last.anchor = last.start();
                    }
                }
                _ => out.push(s),
            }
        }
        if out.is_empty() {
            out.push(Selection::cursor(0));
        }
        self.selections = out;
    }

    // ---- motion ----------------------------------------------------------

    /// Move every cursor. With `extend`, anchors stay put (shift+motion).
    pub fn move_selections(&mut self, motion: Motion, extend: bool) {
        let sels = std::mem::take(&mut self.selections);
        self.selections = sels.into_iter().map(|s| self.moved(s, motion, extend)).collect();
        self.normalize();
    }

    fn moved(&self, s: Selection, motion: Motion, extend: bool) -> Selection {
        let len = self.len_chars();
        // Plain left/right with a selection collapses it to the matching edge.
        if !extend && !s.is_empty() {
            match motion {
                Motion::Left => return Selection::cursor(s.start()),
                Motion::Right => return Selection::cursor(s.end()),
                _ => {}
            }
        }
        let (head, goal) = match motion {
            Motion::Left => (prev_grapheme(&self.rope, s.head), None),
            Motion::Right => (next_grapheme(&self.rope, s.head), None),
            Motion::WordLeft => (self.word_left(s.head), None),
            Motion::WordRight => (self.word_right(s.head), None),
            Motion::DocStart => (0, None),
            Motion::DocEnd => (len, None),
            Motion::LineEnd => {
                let (line, _) = self.line_col(s.head);
                (self.line_start(line) + self.line_len(line), None)
            }
            Motion::LineStart => {
                let (line, col) = self.line_col(s.head);
                let start = self.line_start(line);
                let indent = self.rope.line(line).chars().take_while(|c| *c == ' ' || *c == '\t').count();
                let indent = indent.min(self.line_len(line));
                // Smart home: jump to first non-blank, or to column 0 if already there.
                (if col == indent { start } else { start + indent }, None)
            }
            Motion::Up(n) | Motion::Down(n) => {
                let (line, col) = self.line_col(s.head);
                let goal = s.goal_col.unwrap_or(col);
                let up = matches!(motion, Motion::Up(_));
                let last = self.rope.len_lines() - 1;
                if up && line == 0 {
                    (0, Some(goal))
                } else if !up && line == last {
                    (len, Some(goal))
                } else {
                    let target = if up { line.saturating_sub(n) } else { (line + n).min(last) };
                    (self.offset_of(target, goal), Some(goal))
                }
            }
        };
        Selection { anchor: if extend { s.anchor } else { head }, head, goal_col: goal }
    }

    fn word_left(&self, at: usize) -> usize {
        let mut i = at;
        while i > 0 && char_class(self.rope.char(i - 1)) == CharClass::Space {
            i -= 1;
        }
        if i > 0 {
            let c = char_class(self.rope.char(i - 1));
            while i > 0 && char_class(self.rope.char(i - 1)) == c {
                i -= 1;
            }
        }
        i
    }

    fn word_right(&self, at: usize) -> usize {
        let len = self.len_chars();
        let mut i = at;
        while i < len && char_class(self.rope.char(i)) == CharClass::Space {
            i += 1;
        }
        if i < len {
            let c = char_class(self.rope.char(i));
            while i < len && char_class(self.rope.char(i)) == c {
                i += 1;
            }
        }
        i
    }

    // ---- editing ---------------------------------------------------------

    /// Replace every selection with `text` (typing / paste). `\r\n` is normalized.
    pub fn insert(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let kind = if text.chars().count() == 1 && text != "\n" { TxKind::Typing } else { TxKind::Other };
        self.edit_selections_kind(kind, |_, sel| (sel.start(), sel.end(), text.clone()));
    }

    /// Enter: newline plus the current line's leading whitespace.
    pub fn insert_newline(&mut self) {
        let this = &*self;
        let plans: Vec<String> = this
            .selections
            .iter()
            .map(|s| {
                let (line, col) = this.line_col(s.start());
                let indent: String = this
                    .line(line)
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .take(col)
                    .collect();
                format!("\n{indent}")
            })
            .collect();
        self.edit_selections(|i, sel| (sel.start(), sel.end(), plans[i].clone()));
    }

    /// Tab: insert spaces up to the next tab stop (or replace the selection).
    pub fn insert_tab(&mut self, tab_size: usize) {
        let this = &*self;
        let plans: Vec<String> = this
            .selections
            .iter()
            .map(|s| {
                let (_, col) = this.line_col(s.start());
                " ".repeat(tab_size - col % tab_size)
            })
            .collect();
        self.edit_selections(|i, sel| (sel.start(), sel.end(), plans[i].clone()));
    }

    pub fn delete_backward(&mut self) {
        let rope = self.rope.clone();
        self.edit_selections_kind(TxKind::Deleting, |_, sel| {
            if sel.is_empty() {
                (prev_grapheme(&rope, sel.start()), sel.start(), String::new())
            } else {
                (sel.start(), sel.end(), String::new())
            }
        });
    }

    pub fn delete_forward(&mut self) {
        let rope = self.rope.clone();
        self.edit_selections(|_, sel| {
            if sel.is_empty() {
                (sel.start(), next_grapheme(&rope, sel.start()), String::new())
            } else {
                (sel.start(), sel.end(), String::new())
            }
        });
    }

    pub fn delete_word_backward(&mut self) {
        let this = &*self;
        let starts: Vec<usize> = this.selections.iter().map(|s| this.word_left(s.start())).collect();
        self.edit_selections(|i, sel| {
            if sel.is_empty() {
                (starts[i], sel.start(), String::new())
            } else {
                (sel.start(), sel.end(), String::new())
            }
        });
    }

    /// Delete the selection(s) (Cut). No-op when nothing is selected.
    pub fn delete_selection(&mut self) {
        self.edit_selections(|_, sel| (sel.start(), sel.end(), String::new()));
    }

    /// Core primitive: for each (sorted, non-overlapping) selection compute a
    /// replacement `(start, end, text)`, apply them all as one undo step.
    fn edit_selections(&mut self, f: impl Fn(usize, Selection) -> (usize, usize, String)) {
        self.edit_selections_kind(TxKind::Other, f);
    }

    fn edit_selections_kind(&mut self, kind: TxKind, f: impl Fn(usize, Selection) -> (usize, usize, String)) {
        self.normalize();
        let plans: Vec<(usize, usize, String)> =
            self.selections.iter().enumerate().map(|(i, s)| f(i, *s)).collect();
        if plans.iter().all(|(s, e, t)| s == e && t.is_empty()) {
            return;
        }
        // New cursor positions, accounting for shifts caused by earlier edits.
        let mut after = Vec::with_capacity(plans.len());
        let mut shift: isize = 0;
        for (s, e, t) in &plans {
            let new_len = t.chars().count();
            let pos = (*s as isize + shift) as usize + new_len;
            after.push(Selection::cursor(pos));
            shift += new_len as isize - (*e - *s) as isize;
        }
        self.commit(plans, after, kind);
    }

    /// Apply `plans` (ascending, non-overlapping `(start, end, text)` replacements)
    /// as one undo step, leaving the selections as `after`.
    fn commit(&mut self, plans: Vec<(usize, usize, String)>, after: Vec<Selection>, kind: TxKind) {
        let before = self.selections.clone();
        let now = Instant::now();
        let coalesce = self.can_coalesce(&plans, &before, kind, now);

        // Apply back-to-front so earlier offsets stay valid.
        let mut edits = Vec::with_capacity(plans.len());
        for (s, e, t) in plans.iter().rev() {
            let removed = self.rope.slice(*s..*e).to_string();
            if e > s {
                self.rope.remove(*s..*e);
            }
            if !t.is_empty() {
                self.rope.insert(*s, t);
            }
            edits.push(Edit { at: *s, removed, inserted: t.clone() });
        }

        self.last_edit_at = plans.iter().map(|(s, _, _)| *s).min().unwrap_or(0);
        self.selections = after.clone();
        self.revision += 1;
        self.redo.clear();

        if coalesce {
            // Fold this keystroke into the previous undo step.
            let tx = self.undo.last_mut().expect("checked by can_coalesce");
            let new = edits.pop().expect("single edit");
            let prev = &mut tx.edits[0];
            match kind {
                TxKind::Typing => prev.inserted.push_str(&new.inserted),
                _ => {
                    // Backspacing: the newly removed text sits just before what was removed.
                    prev.at = new.at;
                    prev.removed = new.removed + &prev.removed;
                }
            }
            tx.after = after;
            tx.time = now;
            return;
        }
        let id = self.next_tx;
        self.next_tx += 1;
        self.undo.push(Transaction { id, kind, time: now, edits, before, after });
    }

    fn can_coalesce(
        &self,
        plans: &[(usize, usize, String)],
        before: &[Selection],
        kind: TxKind,
        now: Instant,
    ) -> bool {
        if kind == TxKind::Other || plans.len() != 1 || before.len() != 1 {
            return false;
        }
        let Some(prev) = self.undo.last() else { return false };
        if prev.kind != kind
            || prev.edits.len() != 1
            || prev.id == self.save_tx // never alter the saved state's step
            || now.duration_since(prev.time) > self.group_window
            || prev.after.len() != 1
            || prev.after[0].head != before[0].head
            || !before[0].is_empty()
        {
            return false;
        }
        let (s, e, t) = &plans[0];
        match kind {
            // A character typed at the caret, directly after the previous one.
            TxKind::Typing => s == e && *s == prev.after[0].head && !t.contains('\n') && prev.edits[0].removed.is_empty(),
            // One more character backspaced, directly before what was already removed.
            TxKind::Deleting => t.is_empty() && *e == prev.edits[0].at && prev.edits[0].inserted.is_empty(),
            TxKind::Other => false,
        }
    }

    pub fn undo(&mut self) -> bool {
        let Some(tx) = self.undo.pop() else { return false };
        for e in tx.edits.iter().rev() {
            let n = e.inserted.chars().count();
            if n > 0 {
                self.rope.remove(e.at..e.at + n);
            }
            if !e.removed.is_empty() {
                self.rope.insert(e.at, &e.removed);
            }
        }
        self.last_edit_at = tx.edits.iter().map(|e| e.at).min().unwrap_or(0);
        self.selections = tx.before.clone();
        self.revision += 1;
        self.redo.push(tx);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(tx) = self.redo.pop() else { return false };
        for e in &tx.edits {
            let n = e.removed.chars().count();
            if n > 0 {
                self.rope.remove(e.at..e.at + n);
            }
            if !e.inserted.is_empty() {
                self.rope.insert(e.at, &e.inserted);
            }
        }
        self.last_edit_at = tx.edits.iter().map(|e| e.at).min().unwrap_or(0);
        self.selections = tx.after.clone();
        self.revision += 1;
        self.undo.push(tx);
        true
    }

    /// Line of the earliest change made by the last edit/undo/redo. Everything
    /// above it is untouched, so highlighters can invalidate from here.
    pub fn last_edit_line(&self) -> usize {
        self.line_col(self.last_edit_at).0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Space,
    Word,
    Other,
}

fn char_class(c: char) -> CharClass {
    if c == '\n' || c == '\r' || c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Other
    }
}

fn prev_grapheme(rope: &Rope, at: usize) -> usize {
    if at == 0 {
        return 0;
    }
    boundary(rope, at, false)
}

fn next_grapheme(rope: &Rope, at: usize) -> usize {
    if at >= rope.len_chars() {
        return rope.len_chars();
    }
    boundary(rope, at, true)
}

/// Grapheme boundary search on a bounded window around `at` (keeps this O(1)
/// in document size; graphemes never span more than a handful of chars).
fn boundary(rope: &Rope, at: usize, forward: bool) -> usize {
    const WINDOW: usize = 32;
    let lo = at.saturating_sub(WINDOW);
    let hi = (at + WINDOW).min(rope.len_chars());
    let text = rope.slice(lo..hi).to_string();
    let byte_at = text.char_indices().nth(at - lo).map(|(b, _)| b).unwrap_or(text.len());
    let mut cur = GraphemeCursor::new(byte_at, text.len(), true);
    let res = if forward { cur.next_boundary(&text, 0) } else { cur.prev_boundary(&text, 0) };
    match res {
        Ok(Some(b)) => lo + text[..b].chars().count(),
        _ => {
            if forward {
                hi
            } else {
                lo
            }
        }
    }
}

mod lineops;
pub mod search;

#[cfg(test)]
mod lineops_tests;
#[cfg(test)]
mod tests;
