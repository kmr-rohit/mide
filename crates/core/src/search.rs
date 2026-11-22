//! Find and replace over a [`Buffer`].
//!
//! Matching is line-by-line, so a pattern never spans a line break (`^` and `$`
//! anchor to each line, like Sublime's default). All offsets are *char* offsets.

use std::ops::Range;

use regex::{Regex, RegexBuilder};

use crate::Buffer;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
    pub text: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// Treat `text` as a regular expression (replacements may use `$1`, `${name}`).
    pub regex: bool,
}

/// A compiled [`Query`].
pub struct Matcher {
    re: Regex,
    regex_mode: bool,
}

impl Matcher {
    /// `Ok(None)` for an empty query; `Err` with a readable message for an invalid regex.
    pub fn new(q: &Query) -> Result<Option<Matcher>, String> {
        if q.text.is_empty() {
            return Ok(None);
        }
        let body = if q.regex { q.text.clone() } else { regex::escape(&q.text) };
        let pattern = if q.whole_word { format!(r"\b(?:{body})\b") } else { body };
        let re = RegexBuilder::new(&pattern)
            .case_insensitive(!q.case_sensitive)
            .size_limit(10 << 20)
            .build()
            .map_err(|e| {
                // The regex crate's message is multi-line; the first line says what is wrong.
                let msg = e.to_string();
                msg.lines().last().unwrap_or("invalid pattern").trim().trim_start_matches("error: ").to_owned()
            })?;
        Ok(Some(Matcher { re, regex_mode: q.regex }))
    }

    /// What `matched` becomes when replaced by `template`.
    pub fn replacement(&self, matched: &str, template: &str) -> String {
        if self.regex_mode {
            self.re.replace(matched, template).into_owned()
        } else {
            template.to_owned()
        }
    }

    /// Non-empty matches within one line, as char ranges relative to the line.
    fn line_matches(&self, line: &str) -> Vec<Range<usize>> {
        let ascii = line.is_ascii();
        self.re
            .find_iter(line)
            .filter(|m| !m.is_empty())
            .map(|m| {
                if ascii {
                    m.start()..m.end()
                } else {
                    let s = line[..m.start()].chars().count();
                    s..s + line[m.start()..m.end()].chars().count()
                }
            })
            .collect()
    }
}

/// Every match in document order, stopping after `limit`.
pub fn find_all(buf: &Buffer, m: &Matcher, limit: usize) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    for i in 0..buf.len_lines() {
        let base = buf.line_start(i);
        for r in buf.with_line(i, |l| m.line_matches(l)) {
            out.push(base + r.start..base + r.end);
            if out.len() >= limit {
                return out;
            }
        }
    }
    out
}

/// The first match starting at or after `from`; with `wrap`, continues from the top.
pub fn find_next(buf: &Buffer, m: &Matcher, from: usize, wrap: bool) -> Option<Range<usize>> {
    let n = buf.len_lines();
    let start_line = buf.line_col(from).0;
    for k in 0..=n {
        let i = start_line + k;
        if i >= n && !wrap {
            break;
        }
        let i = i % n;
        let base = buf.line_start(i);
        for r in buf.with_line(i, |l| m.line_matches(l)) {
            let (s, e) = (base + r.start, base + r.end);
            let ok = match k {
                0 => s >= from,
                _ if k == n => s < from, // back on the starting line after wrapping
                _ => true,
            };
            if ok {
                return Some(s..e);
            }
        }
    }
    None
}

/// The last match starting before `before`; with `wrap`, continues from the bottom.
pub fn find_prev(buf: &Buffer, m: &Matcher, before: usize, wrap: bool) -> Option<Range<usize>> {
    let n = buf.len_lines();
    let start_line = buf.line_col(before).0;
    for k in 0..=n {
        let i = start_line as isize - k as isize;
        if i < 0 && !wrap {
            break;
        }
        let i = i.rem_euclid(n as isize) as usize;
        let base = buf.line_start(i);
        let hits = buf.with_line(i, |l| m.line_matches(l));
        let pick = hits.iter().rev().find(|r| match k {
            0 => base + r.start < before,
            _ if k == n => base + r.start >= before,
            _ => true,
        });
        if let Some(r) = pick {
            return Some(base + r.start..base + r.end);
        }
    }
    None
}

/// Replace every match with `template` as a single undo step. Returns how many.
pub fn replace_all(buf: &mut Buffer, m: &Matcher, template: &str) -> usize {
    let ranges = find_all(buf, m, usize::MAX);
    let edits: Vec<(Range<usize>, String)> = ranges
        .into_iter()
        .map(|r| {
            let new = m.replacement(&buf.slice(r.start, r.end), template);
            (r, new)
        })
        .collect();
    let n = edits.len();
    if n > 0 {
        buf.replace_ranges(edits);
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(text: &str) -> Query {
        Query { text: text.into(), ..Query::default() }
    }

    fn m(query: Query) -> Matcher {
        Matcher::new(&query).unwrap().unwrap()
    }

    fn texts(b: &Buffer, rs: &[Range<usize>]) -> Vec<String> {
        rs.iter().map(|r| b.slice(r.start, r.end)).collect()
    }

    #[test]
    fn literal_search_is_case_insensitive_by_default_and_escapes_metachars() {
        let b = Buffer::from_text("Foo foo FOO\nf.o\nfxo");
        assert_eq!(find_all(&b, &m(q("foo")), 100).len(), 3);
        let strict = Query { case_sensitive: true, ..q("foo") };
        assert_eq!(texts(&b, &find_all(&b, &m(strict), 100)), ["foo"]);
        // "." is literal in plain mode.
        assert_eq!(texts(&b, &find_all(&b, &m(q("f.o")), 100)), ["f.o"]);
        let rx = Query { regex: true, ..q("f.o") };
        assert_eq!(find_all(&b, &m(rx), 100).len(), 5); // Foo foo FOO f.o fxo
    }

    #[test]
    fn whole_word_and_empty_query() {
        let b = Buffer::from_text("cat concat cat_x cat");
        let w = Query { whole_word: true, ..q("cat") };
        let hits = find_all(&b, &m(w), 100);
        assert_eq!(hits, [0..3, 17..20]);
        assert!(Matcher::new(&q("")).unwrap().is_none());
    }

    #[test]
    fn invalid_regex_gives_a_readable_error() {
        let e = Matcher::new(&Query { regex: true, ..q("(unclosed") }).err().unwrap();
        assert!(!e.is_empty() && !e.contains('\n'), "got {e:?}");
    }

    #[test]
    fn offsets_are_chars_not_bytes() {
        let b = Buffer::from_text("héllo wörld\nñandú ñ");
        let hits = find_all(&b, &m(q("ñ")), 100);
        assert_eq!(texts(&b, &hits), ["ñ", "ñ"]);
        assert_eq!(hits[0], 12..13);
        assert_eq!(hits[1], 18..19);
    }

    #[test]
    fn next_and_prev_walk_and_wrap() {
        let b = Buffer::from_text("a1 a2\na3\n\na4");
        let a = m(q("a"));
        assert_eq!(find_next(&b, &a, 0, true), Some(0..1));
        assert_eq!(find_next(&b, &a, 1, true), Some(3..4));
        assert_eq!(find_next(&b, &a, 4, true), Some(6..7)); // next line
        assert_eq!(find_next(&b, &a, 11, true), Some(0..1)); // wraps (the last match starts at 10)
        assert_eq!(find_next(&b, &a, 11, false), None);
        assert_eq!(find_prev(&b, &a, 6, true), Some(3..4));
        assert_eq!(find_prev(&b, &a, 0, true), Some(10..11)); // wraps backwards
        assert_eq!(find_prev(&b, &a, 0, false), None);
        // Starting mid-line wraps back to an earlier match on the same line.
        let one = Buffer::from_text("xx a yy");
        assert_eq!(find_next(&one, &a, 6, true), Some(3..4));
    }

    #[test]
    fn limit_caps_the_work() {
        let b = Buffer::from_text(&"x\n".repeat(1000));
        assert_eq!(find_all(&b, &m(q("x")), 10).len(), 10);
    }

    #[test]
    fn replace_all_is_one_undo_step_with_regex_captures() {
        let mut b = Buffer::from_text("john smith\njane doe");
        let rx = m(Query { regex: true, case_sensitive: true, ..q(r"(\w+) (\w+)") });
        assert_eq!(replace_all(&mut b, &rx, "$2, $1"), 2);
        assert_eq!(b.text(), "smith, john\ndoe, jane");
        assert!(b.undo());
        assert_eq!(b.text(), "john smith\njane doe", "one undo reverts every replacement");
        assert!(!b.undo());
    }

    #[test]
    fn plain_replacement_keeps_dollar_signs_literal() {
        let mut b = Buffer::from_text("price price");
        assert_eq!(replace_all(&mut b, &m(q("price")), "$1.00"), 2);
        assert_eq!(b.text(), "$1.00 $1.00");
    }

    #[test]
    fn replacing_with_different_lengths_keeps_later_offsets_right() {
        let mut b = Buffer::from_text("aa-aa-aa");
        replace_all(&mut b, &m(q("aa")), "b");
        assert_eq!(b.text(), "b-b-b");
        replace_all(&mut b, &m(q("b")), "long");
        assert_eq!(b.text(), "long-long-long");
    }
}
