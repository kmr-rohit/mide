//! Syntax highlighting: Sublime `.sublime-syntax` grammars (via syntect) with
//! our own theme layer and an incremental, checkpointed highlighter so edits
//! only re-parse what changed.

pub mod theme;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use syntect::easy::ScopeRangeIterator;
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxSet};

pub use theme::{Rgb, Style, Theme, ThemeError};

/// Parser state is saved every this many lines.
const STRIDE: usize = 64;
/// A window further than this many lines past the parsed region is highlighted from a
/// short warm-up instead of parsing everything in between (which can take seconds).
const MAX_CATCHUP: usize = 2500;
/// Lines parsed above a far window to establish context (comments, strings, ...).
const WARMUP: usize = 250;
/// Lines longer than this (minified files) are not parsed.
const MAX_PARSED_LINE: usize = 10_000;

/// Result of [`DocHighlighter::highlight_window`].
pub struct Highlighted {
    pub lines: Vec<Vec<Span>>,
    /// True when the window was colored from a warm-up rather than the true parse state.
    pub approximate: bool,
}

/// A run of equally-styled text within one line. Offsets are **byte** offsets
/// into the line text (without its newline).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub style: Style,
}

pub struct Syntaxes {
    set: SyntaxSet,
}

impl Syntaxes {
    /// Sublime's default grammars plus the extra set from `bat` (TOML,
    /// TypeScript, Dockerfile, ...).
    pub fn load_default() -> Arc<Self> {
        Arc::new(Self { set: two_face::syntax::extra_newlines() })
    }

    pub fn names(&self) -> Vec<&str> {
        self.set.syntaxes().iter().map(|s| s.name.as_str()).collect()
    }

    /// Pick a syntax by file name, then extension, then first line (shebang);
    /// falls back to "Plain Text".
    pub fn detect(&self, path: Option<&Path>, first_line: &str) -> String {
        if let Some(path) = path {
            let by_name = path.file_name().and_then(|n| n.to_str()).and_then(|n| self.set.find_syntax_by_extension(n));
            let by_ext = path.extension().and_then(|e| e.to_str()).and_then(|e| self.set.find_syntax_by_extension(e));
            if let Some(s) = by_name.or(by_ext) {
                return s.name.clone();
            }
        }
        self.set
            .find_syntax_by_first_line(first_line)
            .unwrap_or_else(|| self.set.find_syntax_plain_text())
            .name
            .clone()
    }
}

#[derive(Clone)]
struct Checkpoint {
    parse: ParseState,
    stack: ScopeStack,
}

/// Highlighter for one document.
pub struct DocHighlighter {
    syntaxes: Arc<Syntaxes>,
    theme: Arc<Theme>,
    syntax_name: String,
    /// `checkpoints[k]` is the state at the start of line `k * STRIDE`.
    checkpoints: Vec<Checkpoint>,
    cache: HashMap<Vec<Scope>, Style>,
}

impl DocHighlighter {
    pub fn new(syntaxes: Arc<Syntaxes>, theme: Arc<Theme>, syntax_name: &str) -> Self {
        let mut h = Self {
            syntaxes,
            theme,
            syntax_name: syntax_name.to_owned(),
            checkpoints: Vec::new(),
            cache: HashMap::new(),
        };
        h.reset();
        h
    }

    fn reset(&mut self) {
        let set = &self.syntaxes.set;
        let syntax = set
            .find_syntax_by_name(&self.syntax_name)
            .unwrap_or_else(|| set.find_syntax_plain_text());
        self.checkpoints = vec![Checkpoint { parse: ParseState::new(syntax), stack: ScopeStack::new() }];
    }

    pub fn set_syntax(&mut self, name: &str) {
        self.syntax_name = name.to_owned();
        self.reset();
    }

    pub fn set_theme(&mut self, theme: Arc<Theme>) {
        self.theme = theme;
        self.cache.clear();
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Call when line `line` (0-based) or anything below it changed.
    pub fn invalidate_from(&mut self, line: usize) {
        // Keep checkpoints whose start line is <= `line`: a checkpoint at the
        // start of the edited line is unaffected by the edit itself.
        let keep = line / STRIDE + 1;
        self.checkpoints.truncate(keep.max(1));
    }

    /// Highlight lines `first .. first + count`. `get_line(i)` must return line
    /// `i` **without** its trailing newline. Returns one span list per line;
    /// spans tile the whole line. (See [`highlight_window`](Self::highlight_window)
    /// for whether the result is exact.)
    pub fn highlight_range(
        &mut self,
        first: usize,
        count: usize,
        get_line: impl FnMut(usize) -> String,
    ) -> Vec<Vec<Span>> {
        self.highlight_window(first, count, get_line).lines
    }

    /// Like `highlight_range`, but says whether the colors are approximate. A window
    /// far beyond anything parsed so far is highlighted from a warm-up of ~250 lines
    /// above it: instant instead of seconds, and right unless a multi-line construct
    /// (block comment, long string) opened before that.
    pub fn highlight_window(
        &mut self,
        first: usize,
        count: usize,
        mut get_line: impl FnMut(usize) -> String,
    ) -> Highlighted {
        let end = first + count;
        let k0 = (first / STRIDE).min(self.checkpoints.len() - 1);
        if first.saturating_sub(k0 * STRIDE) > MAX_CATCHUP {
            return Highlighted { lines: self.highlight_provisional(first, end, &mut get_line), approximate: true };
        }
        let mut cp = self.checkpoints[k0].clone();
        let mut out = Vec::with_capacity(count);

        for line_no in (k0 * STRIDE)..end {
            if line_no % STRIDE == 0 && line_no / STRIDE == self.checkpoints.len() {
                self.checkpoints.push(cp.clone());
            }
            let text = get_line(line_no);
            let want = line_no >= first;
            let spans = self.parse_line(&mut cp, &text, want);
            if want {
                out.push(spans);
            }
        }
        Highlighted { lines: out, approximate: false }
    }

    /// Parse from a fresh state a little above `first`; nothing is checkpointed
    /// because the starting state is a guess.
    fn highlight_provisional(
        &mut self,
        first: usize,
        end: usize,
        get_line: &mut impl FnMut(usize) -> String,
    ) -> Vec<Vec<Span>> {
        let mut cp = self.checkpoints[0].clone(); // the pristine state at line 0
        let mut out = Vec::with_capacity(end - first);
        for line_no in first.saturating_sub(WARMUP)..end {
            let text = get_line(line_no);
            let want = line_no >= first;
            let spans = self.parse_line(&mut cp, &text, want);
            if want {
                out.push(spans);
            }
        }
        out
    }

    fn parse_line(&mut self, cp: &mut Checkpoint, text: &str, want: bool) -> Vec<Span> {
        let base = self.theme.base_style();
        let plain = |len: usize| if len == 0 { Vec::new() } else { vec![Span { start: 0, end: len, style: base }] };

        if text.len() > MAX_PARSED_LINE {
            return if want { plain(text.len()) } else { Vec::new() };
        }
        let with_nl = format!("{text}\n");
        let Ok(ops) = cp.parse.parse_line(&with_nl, &self.syntaxes.set) else {
            return if want { plain(text.len()) } else { Vec::new() };
        };

        let mut spans: Vec<Span> = Vec::new();
        for (range, op) in ScopeRangeIterator::new(&ops, &with_nl) {
            // The op belongs to the *start* of its range: apply it first.
            let _ = cp.stack.apply(op);
            if want && range.start < text.len() {
                let end = range.end.min(text.len());
                if end > range.start {
                    let style = self.style(cp.stack.as_slice());
                    match spans.last_mut() {
                        Some(last) if last.style == style && last.end == range.start => last.end = end,
                        _ => spans.push(Span { start: range.start, end, style }),
                    }
                }
            }
        }
        spans
    }

    fn style(&mut self, stack: &[Scope]) -> Style {
        if let Some(s) = self.cache.get(stack) {
            return *s;
        }
        let s = self.theme.style_for(stack);
        self.cache.insert(stack.to_vec(), s);
        s
    }
}

#[cfg(test)]
mod tests;
