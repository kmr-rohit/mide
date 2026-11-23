//! A single-line text field model (text + caret + the usual editing keys), used by
//! the find/replace bar. No UI in here, so it is fully unit-tested.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineInput {
    text: String,
    /// Caret position in chars.
    caret: usize,
    /// The other end of the selection, when something is selected (the caret is the moving end).
    anchor: Option<usize>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub alt: bool,
    pub cmd: bool,
    pub ctrl: bool,
    pub shift: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Not a key this field handles.
    Ignored,
    /// The caret moved; the text is unchanged.
    Moved,
    /// The text changed.
    Changed,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl LineInput {
    pub fn new(text: &str) -> Self {
        let mut i = Self::default();
        i.set(text);
        i
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    #[cfg(test)]
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// Replace everything, caret at the end. Line breaks are dropped.
    pub fn set(&mut self, text: &str) {
        self.text = text.chars().filter(|c| !matches!(c, '\n' | '\r')).collect();
        self.caret = self.text.chars().count();
        self.anchor = None;
    }

    /// The selected char range `(start, end)`, if any.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        (a != self.caret).then(|| (a.min(self.caret), a.max(self.caret)))
    }

    pub fn selected_text(&self) -> Option<String> {
        let (a, b) = self.selection()?;
        Some(self.text[self.byte(a)..self.byte(b)].to_owned())
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.caret = self.len();
    }

    /// Remove the selection, if any. True when text changed.
    pub fn delete_selection(&mut self) -> bool {
        let Some((a, b)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        self.anchor = None;
        self.remove(a, b)
    }

    /// Text before the selection (or caret), the selected text, and the text after it, for drawing.
    /// `caret_last` says whether the caret sits at the end of the selection.
    pub fn segments(&self) -> (&str, &str, &str, bool) {
        match self.selection() {
            Some((a, b)) => {
                let (x, y) = (self.byte(a), self.byte(b));
                (&self.text[..x], &self.text[x..y], &self.text[y..], self.caret == b)
            }
            None => {
                let (before, after) = self.split();
                (before, "", after, true)
            }
        }
    }

    /// Move the caret, extending the selection when `extend`.
    fn go(&mut self, to: usize, extend: bool) -> Outcome {
        if extend {
            self.anchor.get_or_insert(self.caret);
        } else {
            self.anchor = None;
        }
        self.caret = to;
        if self.anchor == Some(self.caret) {
            self.anchor = None;
        }
        Outcome::Moved
    }

    /// Text before and after the caret, for drawing it.
    pub fn split(&self) -> (&str, &str) {
        self.text.split_at(self.byte(self.caret))
    }

    fn byte(&self, char_idx: usize) -> usize {
        self.text.char_indices().nth(char_idx).map_or(self.text.len(), |(b, _)| b)
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    /// Insert at the caret; control characters and line breaks are dropped.
    pub fn insert(&mut self, s: &str) -> bool {
        let clean: String = s.chars().filter(|c| !c.is_control()).collect();
        if clean.is_empty() {
            return false;
        }
        self.delete_selection();
        let at = self.byte(self.caret);
        self.text.insert_str(at, &clean);
        self.caret += clean.chars().count();
        true
    }

    fn remove(&mut self, from: usize, to: usize) -> bool {
        if from >= to {
            return false;
        }
        let (a, b) = (self.byte(from), self.byte(to));
        self.text.replace_range(a..b, "");
        self.caret = from;
        true
    }

    fn word_left(&self) -> usize {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.caret;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        if i > 0 {
            let w = is_word(chars[i - 1]);
            while i > 0 && !chars[i - 1].is_whitespace() && is_word(chars[i - 1]) == w {
                i -= 1;
            }
        }
        i
    }

    fn word_right(&self) -> usize {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.caret;
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i < chars.len() {
            let w = is_word(chars[i]);
            while i < chars.len() && !chars[i].is_whitespace() && is_word(chars[i]) == w {
                i += 1;
            }
        }
        i
    }

    /// Handle a key press. `key` is the key name ("left", "a", ...); `key_char` is the
    /// text it types, if any.
    pub fn key(&mut self, key: &str, key_char: Option<&str>, m: Mods) -> Outcome {
        let changed = |c: bool| if c { Outcome::Changed } else { Outcome::Ignored };
        let sel = self.selection();
        match key {
            "a" if m.cmd => {
                self.select_all();
                Outcome::Moved
            }
            "backspace" | "delete" if sel.is_some() => changed(self.delete_selection()),
            "backspace" => {
                let from = if m.cmd {
                    0
                } else if m.alt {
                    self.word_left()
                } else {
                    self.caret.saturating_sub(1)
                };
                changed(self.remove(from, self.caret))
            }
            "delete" => {
                let to = if m.cmd {
                    self.len()
                } else if m.alt {
                    self.word_right()
                } else {
                    (self.caret + 1).min(self.len())
                };
                changed(self.remove(self.caret, to))
            }
            "left" => {
                let to = match sel {
                    Some((a, _)) if !m.shift => a, // collapses to the left edge
                    _ if m.cmd => 0,
                    _ if m.alt => self.word_left(),
                    _ => self.caret.saturating_sub(1),
                };
                self.go(to, m.shift)
            }
            "right" => {
                let to = match sel {
                    Some((_, b)) if !m.shift => b,
                    _ if m.cmd => self.len(),
                    _ if m.alt => self.word_right(),
                    _ => (self.caret + 1).min(self.len()),
                };
                self.go(to, m.shift)
            }
            "home" => self.go(0, m.shift),
            "end" => self.go(self.len(), m.shift),
            _ if m.cmd || m.ctrl => Outcome::Ignored,
            _ => match key_char.filter(|s| !s.is_empty()) {
                Some(s) => changed(self.insert(s)),
                None => Outcome::Ignored,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: Mods = Mods { alt: false, cmd: false, ctrl: false, shift: false };
    const ALT: Mods = Mods { alt: true, cmd: false, ctrl: false, shift: false };
    const CMD: Mods = Mods { alt: false, cmd: true, ctrl: false, shift: false };
    const SHIFT: Mods = Mods { alt: false, cmd: false, ctrl: false, shift: true };

    fn typed(i: &mut LineInput, s: &str) {
        for c in s.chars() {
            i.key(&c.to_string(), Some(&c.to_string()), PLAIN);
        }
    }

    #[test]
    fn typing_inserts_at_the_caret() {
        let mut i = LineInput::default();
        typed(&mut i, "helo");
        assert_eq!(i.key("left", None, PLAIN), Outcome::Moved);
        assert_eq!(i.key("l", Some("l"), PLAIN), Outcome::Changed);
        assert_eq!(i.text(), "hello");
        assert_eq!(i.split(), ("hell", "o"));
        assert_eq!(i.caret(), 4);
    }

    #[test]
    fn backspace_and_delete_respect_the_caret_and_edges() {
        let mut i = LineInput::new("abc");
        assert_eq!(i.key("delete", None, PLAIN), Outcome::Ignored); // nothing after the caret
        i.key("backspace", None, PLAIN);
        assert_eq!(i.text(), "ab");
        i.key("home", None, PLAIN);
        assert_eq!(i.key("backspace", None, PLAIN), Outcome::Ignored);
        i.key("delete", None, PLAIN);
        assert_eq!(i.text(), "b");
    }

    #[test]
    fn option_and_command_variants() {
        let mut i = LineInput::new("foo bar_baz  qux");
        i.key("backspace", None, ALT);
        assert_eq!(i.text(), "foo bar_baz  ");
        i.key("backspace", None, ALT);
        assert_eq!(i.text(), "foo ");
        i.key("left", None, ALT);
        assert_eq!(i.caret(), 0);
        i.key("right", None, ALT);
        assert_eq!(i.caret(), 3);
        i.key("backspace", None, CMD);
        assert_eq!(i.text(), " ");
        i.key("end", None, PLAIN);
        i.key("backspace", None, CMD);
        assert!(i.is_empty());
    }

    #[test]
    fn unicode_is_handled_by_chars() {
        let mut i = LineInput::new("é→x");
        assert_eq!(i.caret(), 3);
        i.key("left", None, PLAIN);
        i.key("backspace", None, PLAIN);
        assert_eq!(i.text(), "éx");
        assert_eq!(i.split(), ("é", "x"));
    }

    #[test]
    fn newlines_and_control_chars_never_get_in() {
        let mut i = LineInput::default();
        i.set("a\nb\r");
        assert_eq!(i.text(), "ab");
        assert!(!i.insert("\n\t"));
        assert!(i.insert("x\ny"));
        assert_eq!(i.text(), "abxy");
        // Shortcut chords are not text.
        assert_eq!(i.key("v", Some("v"), CMD), Outcome::Ignored);
        assert_eq!(i.key("shift", None, PLAIN), Outcome::Ignored);
    }

    #[test]
    fn select_all_replace_delete_and_extend() {
        let mut i = LineInput::new("hello world");
        assert_eq!(i.key("a", Some("a"), CMD), Outcome::Moved);
        assert_eq!(i.selected_text().as_deref(), Some("hello world"));
        assert_eq!(i.segments(), ("", "hello world", "", true));
        // Typing replaces the selection.
        assert_eq!(i.key("x", Some("x"), PLAIN), Outcome::Changed);
        assert_eq!(i.text(), "x");
        assert!(i.selection().is_none());

        let mut i = LineInput::new("hello world");
        i.key("a", Some("a"), CMD);
        assert_eq!(i.key("backspace", None, PLAIN), Outcome::Changed);
        assert!(i.is_empty());

        // Shift+arrows extend from the caret; plain arrows collapse to the edge.
        let mut i = LineInput::new("abcdef");
        i.key("left", None, SHIFT);
        i.key("left", None, SHIFT);
        assert_eq!(i.selected_text().as_deref(), Some("ef"));
        assert_eq!(i.segments(), ("abcd", "ef", "", false));
        i.key("left", None, PLAIN);
        assert!(i.selection().is_none());
        assert_eq!(i.caret(), 4);

        // Pasting over a selection replaces it.
        let mut i = LineInput::new("one two");
        i.key("a", Some("a"), CMD);
        assert!(i.insert("three"));
        assert_eq!(i.text(), "three");
    }
}
