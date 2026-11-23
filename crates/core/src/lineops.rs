//! Line-oriented editing commands: duplicate, delete, move, join, indent,
//! comment, new line, and multi-cursor "select next occurrence".

use std::ops::Range;

use crate::{Buffer, Selection, TxKind};

type Plan = (usize, usize, String);

/// Where `p` ends up after applying `edits` (ascending, non-overlapping).
fn map_through(p: usize, edits: &[Plan]) -> usize {
    let mut shift: isize = 0;
    for (s, e, t) in edits {
        let new_len = t.chars().count() as isize;
        if p < *s {
            break;
        }
        if p >= *e {
            shift += new_len - (*e - *s) as isize;
        } else {
            return (*s as isize + shift + new_len) as usize; // inside a replaced span
        }
    }
    (p as isize + shift) as usize
}

fn indent_of(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

impl Buffer {
    fn line_end(&self, l: usize) -> usize {
        self.line_start(l) + self.line_len(l)
    }

    /// Inclusive `(first, last)` line spans touched by the selections, merged.
    /// A selection that ends at column 0 of a later line does not include that line.
    fn selected_line_spans(&self) -> Vec<(usize, usize)> {
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for s in &self.selections {
            let (l0, _) = self.line_col(s.start());
            let (mut l1, c1) = self.line_col(s.end());
            if !s.is_empty() && c1 == 0 && l1 > l0 {
                l1 -= 1;
            }
            match spans.last_mut() {
                Some(last) if l0 <= last.1 + 1 => last.1 = last.1.max(l1),
                _ => spans.push((l0, l1)),
            }
        }
        spans
    }

    /// Apply edits and carry every selection through them.
    fn commit_mapped(&mut self, plans: Vec<Plan>) {
        if plans.is_empty() {
            return;
        }
        let after = self
            .selections
            .iter()
            .map(|s| Selection::new(map_through(s.anchor, &plans), map_through(s.head, &plans)))
            .collect();
        self.commit(plans, after, TxKind::Other);
    }

    /// Does any selection cover more than one line?
    pub fn selection_spans_lines(&self) -> bool {
        self.selections.iter().any(|s| !s.is_empty() && self.line_col(s.start()).0 != self.line_col(s.end()).0)
    }

    /// Replace several ranges at once as one undo step (find/replace "all").
    pub fn replace_ranges(&mut self, edits: Vec<(Range<usize>, String)>) {
        let mut plans: Vec<Plan> = edits.into_iter().map(|(r, t)| (r.start, r.end, t)).collect();
        plans.sort_by_key(|p| p.0);
        let Some(first) = plans.first().map(|p| p.0) else { return };
        let _ = first;
        let mut shift: isize = 0;
        let mut caret = 0;
        for (s, e, t) in &plans {
            let nl = t.chars().count();
            caret = (*s as isize + shift) as usize + nl;
            shift += nl as isize - (*e - *s) as isize;
        }
        self.commit(plans, vec![Selection::cursor(caret)], TxKind::Other);
    }

    pub fn goto_line(&mut self, line: usize, col: usize) {
        let at = self.offset_of(line.saturating_sub(1), col.saturating_sub(1));
        self.set_cursor(at);
    }

    /// ⌘L: select whole lines; again to take the next line too.
    pub fn select_line(&mut self) {
        let n = self.len_lines();
        let mut out = Vec::new();
        for s in self.selections.clone() {
            let (l0, _) = self.line_col(s.start());
            let (mut l1, c1) = self.line_col(s.end());
            if !s.is_empty() && c1 == 0 && l1 > l0 {
                l1 -= 1;
            }
            let start = self.line_start(l0);
            let after = |l: usize| if l + 1 < n { self.line_start(l + 1) } else { self.len_chars() };
            let mut end = after(l1);
            if s.start() == start && s.end() == end && l1 + 1 < n {
                end = after(l1 + 1);
            }
            out.push(Selection::new(start, end));
        }
        self.set_selections(out);
    }

    /// ⌘⇧D: copy the selected lines below, moving the selection into the copy.
    pub fn duplicate_line(&mut self) {
        let spans = self.selected_line_spans();
        let n = self.len_lines();
        let mut plans = Vec::new();
        for (a, b) in &spans {
            let block = (*a..=*b).map(|l| self.line(l)).collect::<Vec<_>>().join("\n");
            plans.push(if b + 1 < n {
                let pos = self.line_start(b + 1);
                (pos, pos, format!("{block}\n"))
            } else {
                let pos = self.len_chars();
                (pos, pos, format!("\n{block}"))
            });
        }
        let lens: Vec<usize> = plans.iter().map(|p| p.2.chars().count()).collect();
        let after = self
            .selections
            .iter()
            .map(|s| {
                let l = self.line_col(s.start()).0;
                let i = spans.iter().position(|(a, b)| (*a..=*b).contains(&l)).unwrap_or(0);
                let shift: usize = lens[..=i].iter().sum();
                Selection::new(s.anchor + shift, s.head + shift)
            })
            .collect();
        self.commit(plans, after, TxKind::Other);
    }

    /// ⌃⇧K: delete the selected lines.
    pub fn delete_line(&mut self) {
        let spans = self.selected_line_spans();
        let n = self.len_lines();
        let mut plans = Vec::new();
        for (a, b) in &spans {
            plans.push(if b + 1 < n {
                (self.line_start(*a), self.line_start(b + 1), String::new())
            } else if *a > 0 {
                // Last line has no newline of its own: take the one before it.
                (self.line_start(*a) - 1, self.len_chars(), String::new())
            } else {
                (0, self.len_chars(), String::new())
            });
        }
        let mut shift: isize = 0;
        let mut after = Vec::new();
        for (s, e, _) in &plans {
            after.push(Selection::cursor((*s as isize + shift) as usize));
            shift -= (*e - *s) as isize;
        }
        self.commit(plans, after, TxKind::Other);
    }

    /// ⌃⌘↑ / ⌃⌘↓: swap the selected lines with the neighbour above/below.
    pub fn move_lines(&mut self, up: bool) {
        let n = self.len_lines();
        let mut plans: Vec<Plan> = Vec::new();
        let mut deltas: Vec<((usize, usize), isize)> = Vec::new();
        let mut last_end: Option<usize> = None;
        for (a, b) in self.selected_line_spans() {
            let (ra, rb) = if up {
                if a == 0 {
                    continue;
                }
                (a - 1, b)
            } else {
                if b + 1 >= n {
                    continue;
                }
                (a, b + 1)
            };
            let start = self.line_start(ra);
            if last_end.is_some_and(|e| start <= e) {
                continue; // overlaps the previous move
            }
            let block: Vec<String> = (a..=b).map(|l| self.line(l)).collect();
            let (text, delta) = if up {
                let neighbour = self.line(a - 1);
                let d = -(neighbour.chars().count() as isize + 1);
                (block.into_iter().chain([neighbour]).collect::<Vec<_>>().join("\n"), d)
            } else {
                let neighbour = self.line(b + 1);
                let d = neighbour.chars().count() as isize + 1;
                ([neighbour].into_iter().chain(block).collect::<Vec<_>>().join("\n"), d)
            };
            let end = self.line_end(rb);
            last_end = Some(end);
            plans.push((start, end, text));
            deltas.push(((a, b), delta));
        }
        if plans.is_empty() {
            return;
        }
        let after = self
            .selections
            .iter()
            .map(|s| {
                let l = self.line_col(s.start()).0;
                let d = deltas.iter().find(|((a, b), _)| (*a..=*b).contains(&l)).map_or(0, |(_, d)| *d);
                Selection::new((s.anchor as isize + d) as usize, (s.head as isize + d) as usize)
            })
            .collect();
        self.commit(plans, after, TxKind::Other);
    }

    /// ⌘J: join each selected line with the next (or all selected lines together).
    pub fn join_lines(&mut self) {
        let n = self.len_lines();
        let mut plans: Vec<Plan> = Vec::new();
        let mut join_points = Vec::new();
        for (a, b) in self.selected_line_spans() {
            let b = if a == b {
                if a + 1 >= n {
                    continue;
                }
                a + 1
            } else {
                b
            };
            let first = self.line(a);
            let mut out = first.clone();
            for l in a + 1..=b {
                let t = self.line(l);
                let t = t.trim_start();
                if t.is_empty() {
                    continue;
                }
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str(t);
            }
            join_points.push(first.chars().count());
            plans.push((self.line_start(a), self.line_end(b), out));
        }
        if plans.is_empty() {
            return;
        }
        let mut shift: isize = 0;
        let mut after = Vec::new();
        for ((s, e, t), jp) in plans.iter().zip(&join_points) {
            after.push(Selection::cursor((*s as isize + shift) as usize + jp));
            shift += t.chars().count() as isize - (*e - *s) as isize;
        }
        self.commit(plans, after, TxKind::Other);
    }

    /// ⌘] / Tab on a multi-line selection: indent the selected lines.
    pub fn indent_lines(&mut self, tab_size: usize) {
        let pad = if self.indent == crate::Indent::Tabs { "\t".to_owned() } else { " ".repeat(tab_size) };
        let mut plans = Vec::new();
        for (a, b) in self.selected_line_spans() {
            for l in a..=b {
                if self.line_len(l) > 0 {
                    let p = self.line_start(l);
                    plans.push((p, p, pad.clone()));
                }
            }
        }
        self.commit_mapped(plans);
    }

    /// ⌘[ / ⇧Tab: remove one level of indentation from the selected lines.
    pub fn outdent_lines(&mut self, tab_size: usize) {
        let mut plans = Vec::new();
        for (a, b) in self.selected_line_spans() {
            for l in a..=b {
                let line = self.line(l);
                let take = if line.starts_with('\t') {
                    1
                } else {
                    line.chars().take(tab_size).take_while(|c| *c == ' ').count()
                };
                if take > 0 {
                    let p = self.line_start(l);
                    plans.push((p, p + take, String::new()));
                }
            }
        }
        self.commit_mapped(plans);
    }

    /// ⌘/: comment the selected lines with `token`, or uncomment if all already are.
    pub fn toggle_comment(&mut self, token: &str) {
        let mut lines: Vec<usize> = Vec::new();
        for (a, b) in self.selected_line_spans() {
            lines.extend((a..=b).filter(|l| !self.line(*l).trim().is_empty()));
        }
        if lines.is_empty() || token.is_empty() {
            return;
        }
        let tok_len = token.chars().count();
        let all_commented = lines.iter().all(|l| self.line(*l).trim_start().starts_with(token));
        let mut plans: Vec<Plan> = Vec::new();
        if all_commented {
            for l in lines {
                let line = self.line(l);
                let at = self.line_start(l) + indent_of(&line);
                let space = line.chars().nth(indent_of(&line) + tok_len) == Some(' ');
                plans.push((at, at + tok_len + space as usize, String::new()));
            }
        } else {
            let col = lines.iter().map(|l| indent_of(&self.line(*l))).min().unwrap_or(0);
            for l in lines {
                let at = self.line_start(l) + col;
                plans.push((at, at, format!("{token} ")));
            }
        }
        self.commit_mapped(plans);
    }

    /// ⌘↩: open a new line below, keeping the indentation, and move there.
    pub fn insert_line_after(&mut self) {
        self.open_line(false);
    }

    /// ⌘⇧↩: open a new line above.
    pub fn insert_line_before(&mut self) {
        self.open_line(true);
    }

    fn open_line(&mut self, before: bool) {
        let mut lines: Vec<usize> = self.selections.iter().map(|s| self.line_col(s.head).0).collect();
        lines.dedup();
        let mut plans = Vec::new();
        let mut carets = Vec::new();
        for l in lines {
            let indent: String = self.line(l).chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            let il = indent.chars().count();
            if before {
                let p = self.line_start(l);
                plans.push((p, p, format!("{indent}\n")));
                carets.push((p, il));
            } else {
                let p = self.line_end(l);
                plans.push((p, p, format!("\n{indent}")));
                carets.push((p, 1 + il));
            }
        }
        let mut shift: isize = 0;
        let mut after = Vec::new();
        for ((s, _, t), (_, off)) in plans.iter().zip(&carets) {
            after.push(Selection::cursor((*s as isize + shift) as usize + off));
            shift += t.chars().count() as isize;
        }
        self.commit(plans, after, TxKind::Other);
    }

    // ---- multi-cursor ------------------------------------------------------------------

    /// Char ranges of every exact (case-sensitive) occurrence of `needle`.
    fn occurrences(&self, needle: &str) -> Vec<Range<usize>> {
        if needle.is_empty() {
            return Vec::new();
        }
        let hay = self.rope.to_string();
        let width = needle.chars().count();
        let (mut chars, mut last_byte, mut out) = (0usize, 0usize, Vec::new());
        for (byte, _) in hay.match_indices(needle) {
            chars += hay[last_byte..byte].chars().count();
            last_byte = byte;
            out.push(chars..chars + width);
        }
        out
    }

    /// ⌘D: select the word under the cursor; again to add the next occurrence.
    pub fn add_next_occurrence(&mut self) {
        if self.selections.iter().all(Selection::is_empty) {
            let sels = self
                .selections
                .clone()
                .into_iter()
                .map(|s| {
                    let (a, b) = self.word_range(s.head);
                    Selection::new(a, b)
                })
                .collect();
            self.set_selections(sels);
            return;
        }
        let primary = *self.selections.last().expect("never empty");
        let needle = self.slice(primary.start(), primary.end());
        let all = self.occurrences(&needle);
        let taken = |r: &Range<usize>| self.selections.iter().any(|s| s.start() < r.end && r.start < s.end());
        let next = all
            .iter()
            .find(|r| r.start >= primary.end() && !taken(r))
            .or_else(|| all.iter().find(|r| !taken(r)));
        if let Some(r) = next {
            self.selections.push(Selection::new(r.start, r.end));
            self.normalize();
        }
    }

    /// ⌃⌘G: select every occurrence of the selected text (or the word at the cursor).
    pub fn select_all_occurrences(&mut self) {
        let primary = *self.selections.last().expect("never empty");
        let (a, b) = if primary.is_empty() { self.word_range(primary.head) } else { (primary.start(), primary.end()) };
        let needle = self.slice(a, b);
        let mut all = self.occurrences(&needle);
        if primary.is_empty() {
            // Started from a bare cursor: match the whole word, not text inside longer words.
            let is_word = |i: usize| crate::char_class(self.rope.char(i)) == crate::CharClass::Word;
            all.retain(|r| (r.start == 0 || !is_word(r.start - 1)) && (r.end >= self.len_chars() || !is_word(r.end)));
        }
        if all.is_empty() {
            return;
        }
        self.set_selections(all.into_iter().map(|r| Selection::new(r.start, r.end)).collect());
    }
}
