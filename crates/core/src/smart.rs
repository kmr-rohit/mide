//! Editing that understands code a little: the file's indentation style, auto-indent after
//! `{` or `:`, and auto-closing brackets and quotes.

use crate::{Buffer, Selection, TxKind};

/// How a file indents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indent {
    Tabs,
    Spaces(usize),
}

impl Indent {
    /// One level of indentation as text.
    pub fn unit(self) -> String {
        match self {
            Indent::Tabs => "\t".to_owned(),
            Indent::Spaces(n) => " ".repeat(n),
        }
    }

    /// Width of a level in columns (a tab counts as 4 for display purposes).
    pub fn width(self) -> usize {
        match self {
            Indent::Tabs => 4,
            Indent::Spaces(n) => n,
        }
    }

    /// For the status bar.
    pub fn label(self) -> String {
        match self {
            Indent::Tabs => "Tab Size: 4 (tabs)".to_owned(),
            Indent::Spaces(n) => format!("Spaces: {n}"),
        }
    }
}

/// Guess the indentation style from the first lines of a file. `None` when there is no
/// indented line to learn from (callers keep their default).
pub fn detect_indent(text: &str) -> Option<Indent> {
    let mut tab_lines = 0usize;
    let mut votes = [0usize; 9];
    let mut prev = 0usize;
    for line in text.lines().take(2000) {
        if line.trim().is_empty() {
            continue;
        }
        let trimmed = line.trim_start_matches(' ');
        let spaces = line.len() - trimmed.len();
        if line.starts_with('\t') {
            tab_lines += 1;
            prev = 0;
            continue;
        }
        // " * text" continuation lines of /* */ comments are not indentation.
        if trimmed.starts_with('*') && spaces == 1 {
            continue;
        }
        let delta = spaces.abs_diff(prev);
        if (2..=8).contains(&delta) {
            votes[delta] += 1;
        }
        prev = spaces;
    }
    let space_votes: usize = votes.iter().sum();
    if tab_lines > 0 && tab_lines >= space_votes {
        return Some(Indent::Tabs);
    }
    // Only 2, 3, 4 and 8 are plausible indent widths; ignore odd deltas from alignment.
    let best = [2usize, 4, 3, 8].into_iter().max_by_key(|&n| (votes[n], std::cmp::Reverse(n)))?;
    (votes[best] > 0).then_some(Indent::Spaces(best))
}

fn closer_of(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' | '\'' | '`' => Some(c),
        _ => None,
    }
}

fn is_quote(c: char) -> bool {
    matches!(c, '"' | '\'' | '`')
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Buffer {
    pub fn indent(&self) -> Indent {
        self.indent
    }

    pub fn set_indent(&mut self, indent: Indent) {
        self.indent = indent;
    }

    fn char_at(&self, i: usize) -> Option<char> {
        (i < self.rope.len_chars()).then(|| self.rope.char(i))
    }

    /// Enter with auto-indent: keep the line's indentation, add a level after `{`, `[`, `(`
    /// (and after `:` when `colon_indents`, for Python/YAML), and open up `{|}` into three lines.
    pub fn insert_newline_auto(&mut self, colon_indents: bool) {
        self.normalize();
        let unit = self.indent.unit();
        let mut plans = Vec::new();
        let mut cursor_in_text = Vec::new();
        for s in &self.selections {
            let (line, col) = self.line_col(s.start());
            let text = self.line(line);
            let lead: String = text.chars().take_while(|c| *c == ' ' || *c == '\t').take(col).collect();
            let before: String = text.chars().take(col).collect();
            let last = before.trim_end().chars().last();
            let opens = matches!(last, Some('{' | '[' | '('));
            let colon = colon_indents && last == Some(':');
            let next = self.char_at(s.end());
            let closes = matches!((last, next), (Some('{'), Some('}')) | (Some('['), Some(']')) | (Some('('), Some(')')));
            let (insert, cursor) = if opens && closes {
                let mid = format!("\n{lead}{unit}");
                let n = mid.chars().count();
                (format!("{mid}\n{lead}"), n)
            } else if opens || colon {
                let t = format!("\n{lead}{unit}");
                let n = t.chars().count();
                (t, n)
            } else {
                let t = format!("\n{lead}");
                let n = t.chars().count();
                (t, n)
            };
            plans.push((s.start(), s.end(), insert));
            cursor_in_text.push(cursor);
        }
        let mut after = Vec::with_capacity(plans.len());
        let mut shift: isize = 0;
        for ((s, e, t), cur) in plans.iter().zip(cursor_in_text) {
            after.push(Selection::cursor((*s as isize + shift) as usize + cur));
            shift += t.chars().count() as isize - (*e - *s) as isize;
        }
        self.commit(plans, after, TxKind::Other);
    }

    /// Typing one character with auto-closing. Returns `false` when the character is not
    /// special here and the caller should insert it normally.
    pub fn type_char(&mut self, c: char) -> bool {
        let Some(close) = closer_of(c) else {
            // Typing a closer over an identical one just steps past it.
            if matches!(c, ')' | ']' | '}') {
                return self.step_over(c);
            }
            return false;
        };
        // A quote that matches the next character closes instead of opening.
        if is_quote(c) && self.step_over(c) {
            return true;
        }
        self.normalize();
        let mut plans = Vec::new();
        let mut after_in_text = Vec::new();
        let mut any_pair = false;
        for s in &self.selections {
            if !s.is_empty() {
                // Wrap the selection and keep it selected.
                let inner = self.slice(s.start(), s.end());
                plans.push((s.start(), s.end(), format!("{c}{inner}{close}")));
                after_in_text.push((1usize, Some(inner.chars().count())));
                any_pair = true;
                continue;
            }
            let at = s.start();
            let prev = at.checked_sub(1).and_then(|i| self.char_at(i));
            let next = self.char_at(at);
            let next_ok = next.is_none_or(|n| n.is_whitespace() || matches!(n, ')' | ']' | '}' | ',' | ';' | ':'));
            let prev_ok = !is_quote(c) || !prev.is_some_and(is_word);
            if next_ok && prev_ok {
                plans.push((at, at, format!("{c}{close}")));
                after_in_text.push((1, None));
                any_pair = true;
            } else {
                plans.push((at, at, c.to_string()));
                after_in_text.push((1, None));
            }
        }
        if !any_pair {
            return false;
        }
        let mut after = Vec::with_capacity(plans.len());
        let mut shift: isize = 0;
        for ((s, e, t), (off, keep)) in plans.iter().zip(after_in_text) {
            let base = (*s as isize + shift) as usize + off;
            after.push(match keep {
                Some(n) => Selection::new(base, base + n),
                None => Selection::cursor(base),
            });
            shift += t.chars().count() as isize - (*e - *s) as isize;
        }
        self.commit(plans, after, TxKind::Other);
        true
    }

    /// If every caret sits right before `c`, move all of them past it.
    fn step_over(&mut self, c: char) -> bool {
        if !self.selections.iter().all(|s| s.is_empty() && self.char_at(s.start()) == Some(c)) {
            return false;
        }
        let moved = self.selections.iter().map(|s| Selection::cursor(s.start() + 1)).collect();
        self.set_selections(moved);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_spaces_tabs_and_ignores_comment_stars() {
        assert_eq!(detect_indent("a:\n  b:\n    c: 1\n  d: 2\n"), Some(Indent::Spaces(2)));
        assert_eq!(detect_indent("fn a() {\n    x;\n    if y {\n        z;\n    }\n}\n"), Some(Indent::Spaces(4)));
        assert_eq!(detect_indent("a {\n\tb;\n\tc {\n\t\td;\n\t}\n}\n"), Some(Indent::Tabs));
        assert_eq!(detect_indent("/*\n * doc\n */\nx\n"), None);
        assert_eq!(detect_indent("no indentation\nat all\n"), None);
    }

    #[test]
    fn enter_adds_a_level_after_an_opener_and_opens_braces() {
        let mut b = Buffer::from_text("fn a() {");
        b.set_cursor(8);
        b.insert_newline_auto(false);
        assert_eq!(b.text(), "fn a() {\n    ");
        let mut b = Buffer::from_text("  v = {}");
        b.set_cursor(7);
        b.insert_newline_auto(false);
        assert_eq!(b.text(), "  v = {\n      \n  }");
        let (line, col) = b.line_col(b.selections()[0].head);
        assert_eq!((line, col), (1, 6));
    }

    #[test]
    fn enter_after_colon_only_for_languages_that_want_it() {
        let mut b = Buffer::from_text("if x:");
        b.set_cursor(5);
        b.insert_newline_auto(true);
        assert_eq!(b.text(), "if x:\n    ");
        let mut b = Buffer::from_text("a: b:");
        b.set_cursor(5);
        b.insert_newline_auto(false);
        assert_eq!(b.text(), "a: b:\n");
    }

    #[test]
    fn enter_uses_tabs_when_the_file_does() {
        let mut b = Buffer::from_text("{");
        b.set_indent(Indent::Tabs);
        b.set_cursor(1);
        b.insert_newline_auto(false);
        assert_eq!(b.text(), "{\n\t");
    }

    #[test]
    fn brackets_close_and_step_over() {
        let mut b = Buffer::default();
        assert!(b.type_char('('));
        assert_eq!(b.text(), "()");
        assert!(b.type_char(')'));
        assert_eq!(b.text(), "()");
        assert_eq!(b.selections()[0].head, 2);
        // Not before a word: `(x` stays a single bracket.
        let mut b = Buffer::from_text("word");
        b.set_cursor(0);
        assert!(!b.type_char('('));
    }

    #[test]
    fn quotes_do_not_pair_inside_words() {
        let mut b = Buffer::from_text("don");
        b.set_cursor(3);
        assert!(!b.type_char('\''));
        let mut b = Buffer::default();
        assert!(b.type_char('"'));
        assert_eq!(b.text(), "\"\"");
        assert!(b.type_char('"'));
        assert_eq!(b.text(), "\"\"");
        assert_eq!(b.selections()[0].head, 2);
    }

    #[test]
    fn typing_an_opener_wraps_the_selection() {
        let mut b = Buffer::from_text("hello");
        b.set_selections(vec![Selection::new(0, 5)]);
        assert!(b.type_char('['));
        assert_eq!(b.text(), "[hello]");
        assert_eq!((b.selections()[0].start(), b.selections()[0].end()), (1, 6));
    }

    #[test]
    fn backspace_removes_an_empty_pair() {
        let mut b = Buffer::default();
        b.type_char('{');
        b.delete_backward();
        assert_eq!(b.text(), "");
        let mut b = Buffer::from_text("(x)");
        b.set_cursor(2);
        b.delete_backward();
        assert_eq!(b.text(), "()");
    }

    #[test]
    fn multi_cursor_pairs() {
        let mut b = Buffer::from_text("\n\n");
        b.set_selections(vec![Selection::cursor(0), Selection::cursor(1), Selection::cursor(2)]);
        assert!(b.type_char('['));
        assert_eq!(b.text(), "[]\n[]\n[]");
        assert_eq!(b.selections().iter().map(|s| s.head).collect::<Vec<_>>(), vec![1, 4, 7]);
    }
}
