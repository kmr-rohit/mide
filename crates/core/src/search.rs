//! Find and replace over a [`Buffer`].
//!
//! Matching is line-by-line, so a pattern never spans a line break (`^` and `$`
//! anchor to each line, like Sublime's default). All offsets are *char* offsets.

use std::ops::Range;

use regex::{Regex, RegexBuilder};
use ropey::Rope;

use crate::Buffer;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
    pub text: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// Treat `text` as a regular expression (replacements may use `$1`, `${name}`).
    pub regex: bool,
}

/// A compiled [`Query`]. Cheap to clone (the regexes are shared).
#[derive(Clone)]
pub struct Matcher {
    re: Regex,
    /// The same pattern in multi-line + CRLF mode, for scanning many lines in one pass.
    block: Regex,
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
        let block = RegexBuilder::new(&format!("(?mR){pattern}"))
            .case_insensitive(!q.case_sensitive)
            .size_limit(10 << 20)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Some(Matcher { re, block, regex_mode: q.regex }))
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
    pub fn line_matches(&self, line: &str) -> Vec<Range<usize>> {
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

// ---- the scanner -----------------------------------------------------------------------------------
//
// Whole-file searches copy big blocks of lines out of the rope into one contiguous string and run the
// regex once per block (instead of once per line), spread over all cores. A match that would span a
// line break makes that block fall back to the strict line-by-line path, so semantics don't change.

/// Chars per block copied out of the rope.
const BLOCK_CHARS: usize = 1 << 20;
/// Below this many chars a search runs on the calling thread.
const PAR_MIN_CHARS: usize = 300_000;

#[derive(Clone, Copy)]
enum Pick {
    First(usize),
    Last,
}

struct Scan<'a> {
    rope: &'a Rope,
    m: &'a Matcher,
    /// Only matches whose start is in `min..max` count.
    min: usize,
    max: usize,
    pick: Pick,
}

impl Scan<'_> {
    fn keep(&self, r: Range<usize>, out: &mut Vec<Range<usize>>) -> bool {
        if r.start < self.min || r.start >= self.max {
            return false;
        }
        match self.pick {
            Pick::First(n) => {
                out.push(r);
                out.len() >= n
            }
            Pick::Last => {
                out.clear();
                out.push(r);
                false
            }
        }
    }

    /// Matches in lines `lo..hi`. `stop` is polled between blocks.
    fn lines(&self, lo: usize, hi: usize, stop: &dyn Fn() -> bool) -> Vec<Range<usize>> {
        let rope = self.rope;
        let mut out = Vec::new();
        let mut text = String::new();
        let mut a = lo;
        while a < hi && !stop() {
            let ca = rope.line_to_char(a);
            let mut b = rope.char_to_line((ca + BLOCK_CHARS).min(rope.len_chars()));
            b = b.clamp(a + 1, hi);
            let cb = rope.line_to_char(b);
            if cb <= self.min && ca < self.min && b < hi {
                a = b;
                continue; // entirely before the first line that can match
            }
            if ca >= self.max {
                break;
            }
            text.clear();
            for chunk in rope.slice(ca..cb).chunks() {
                text.push_str(chunk);
            }
            let (len, last) = (out.len(), out.last().cloned());
            match self.block(&text, ca, &mut out) {
                Some(true) => return out,
                Some(false) => {}
                None => {
                    // Drop what the aborted block pushed, then redo it strictly line by line.
                    out.truncate(len);
                    if matches!(self.pick, Pick::Last) {
                        out.clear();
                        out.extend(last);
                    }
                    if self.slow(a, b, &mut out) {
                        return out;
                    }
                }
            }
            a = b;
        }
        out
    }

    /// One contiguous block. `None` when a match crosses a line break; `Some(true)` when done.
    fn block(&self, text: &str, base: usize, out: &mut Vec<Range<usize>>) -> Option<bool> {
        let ascii = text.is_ascii();
        let (mut last_byte, mut last_char) = (0usize, 0usize);
        for mt in self.m.block.find_iter(text) {
            if mt.is_empty() {
                continue;
            }
            let hit = mt.as_str();
            if hit.contains(['\n', '\r']) {
                return None;
            }
            let r = if ascii {
                base + mt.start()..base + mt.end()
            } else {
                last_char += text[last_byte..mt.start()].chars().count();
                last_byte = mt.start();
                base + last_char..base + last_char + hit.chars().count()
            };
            if self.keep(r, out) {
                return Some(true);
            }
        }
        Some(false)
    }

    /// Strict per-line matching for lines `a..b`.
    fn slow(&self, a: usize, b: usize, out: &mut Vec<Range<usize>>) -> bool {
        for i in a..b {
            let base = self.rope.line_to_char(i);
            for r in with_line(self.rope, i, |l| self.m.line_matches(l)) {
                if self.keep(base + r.start..base + r.end, out) {
                    return true;
                }
            }
        }
        false
    }

    /// Scan lines `lo..hi`, split across threads. One result vec per segment, in document order.
    fn par(&self, lo: usize, hi: usize) -> Vec<Vec<Range<usize>>> {
        use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
        if lo >= hi {
            return Vec::new();
        }
        let rope = self.rope;
        let (c0, c1) = (rope.line_to_char(lo), rope.line_to_char(hi));
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(16);
        if c1 - c0 < PAR_MIN_CHARS || threads < 2 {
            return vec![self.lines(lo, hi, &|| false)];
        }
        let mut bounds = vec![lo];
        for i in 1..threads {
            let l = rope.char_to_line(c0 + (c1 - c0) * i / threads).clamp(*bounds.last().unwrap(), hi);
            if l > *bounds.last().unwrap() {
                bounds.push(l);
            }
        }
        bounds.push(hi);
        let segs = bounds.len() - 1;
        // `best` = the segment nearest the front (First) or back (Last) known to have a hit.
        let first = matches!(self.pick, Pick::First(1));
        let last = matches!(self.pick, Pick::Last);
        let best = AtomicUsize::new(if last { 0 } else { usize::MAX });
        let hit = AtomicUsize::new(0); // for Last: 1 + highest segment with a hit
        let mut results: Vec<Vec<Range<usize>>> = vec![Vec::new(); segs];
        std::thread::scope(|sc| {
            for (i, slot) in results.iter_mut().enumerate() {
                let (a, b) = (bounds[i], bounds[i + 1]);
                let (best, hit) = (&best, &hit);
                sc.spawn(move || {
                    let stop = move || {
                        (first && best.load(Relaxed) < i) || (last && hit.load(Relaxed) > i + 1)
                    };
                    *slot = self.lines(a, b, &stop);
                    if !slot.is_empty() {
                        if first {
                            best.fetch_min(i, Relaxed);
                        }
                        if last {
                            hit.fetch_max(i + 1, Relaxed);
                        }
                    }
                });
            }
        });
        results
    }
}

/// Line `i` of `rope` without its terminator.
fn with_line<R>(rope: &Rope, i: usize, f: impl FnOnce(&str) -> R) -> R {
    let slice = rope.line(i);
    match slice.as_str() {
        Some(s) => f(s.trim_end_matches(['\n', '\r'])),
        None => f(slice.to_string().trim_end_matches(['\n', '\r'])),
    }
}

fn line_of(rope: &Rope, offset: usize) -> usize {
    rope.char_to_line(offset.min(rope.len_chars()))
}

/// Every match in document order, stopping after `limit`.
pub fn find_all(buf: &Buffer, m: &Matcher, limit: usize) -> Vec<Range<usize>> {
    find_all_in(buf.rope(), m, limit)
}

/// The first match starting at or after `from`; with `wrap`, continues from the top.
pub fn find_next(buf: &Buffer, m: &Matcher, from: usize, wrap: bool) -> Option<Range<usize>> {
    find_next_in(buf.rope(), m, from, wrap)
}

/// The last match starting before `before`; with `wrap`, continues from the bottom.
pub fn find_prev(buf: &Buffer, m: &Matcher, before: usize, wrap: bool) -> Option<Range<usize>> {
    find_prev_in(buf.rope(), m, before, wrap)
}

// The `_in` variants take a rope, so a search can run on a snapshot (`Rope::clone` is O(1)) on another thread.

pub fn find_all_in(rope: &Rope, m: &Matcher, limit: usize) -> Vec<Range<usize>> {
    let scan = Scan { rope, m, min: 0, max: usize::MAX, pick: Pick::First(limit) };
    let mut out: Vec<Range<usize>> = scan.par(0, rope.len_lines()).into_iter().flatten().collect();
    out.truncate(limit);
    out
}

pub fn find_next_in(rope: &Rope, m: &Matcher, from: usize, wrap: bool) -> Option<Range<usize>> {
    let n = rope.len_lines();
    let sl = line_of(rope, from);
    let scan = Scan { rope, m, min: from, max: usize::MAX, pick: Pick::First(1) };
    if let Some(r) = scan.par(sl, n).into_iter().flatten().next() {
        return Some(r);
    }
    if !wrap {
        return None;
    }
    // Back on the starting line after wrapping, only matches before `from` are new.
    let scan = Scan { rope, m, min: 0, max: from, pick: Pick::First(1) };
    scan.par(0, (sl + 1).min(n)).into_iter().flatten().next()
}

pub fn find_prev_in(rope: &Rope, m: &Matcher, before: usize, wrap: bool) -> Option<Range<usize>> {
    let n = rope.len_lines();
    let sl = line_of(rope, before);
    let scan = Scan { rope, m, min: 0, max: before, pick: Pick::Last };
    if let Some(r) = scan.par(0, (sl + 1).min(n)).into_iter().flatten().last() {
        return Some(r);
    }
    if !wrap {
        return None;
    }
    let scan = Scan { rope, m, min: before, max: usize::MAX, pick: Pick::Last };
    scan.par(sl, n).into_iter().flatten().last()
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


    /// The parallel block scanner must agree with strict line-by-line matching.
    #[test]
    fn big_buffer_scan_matches_line_by_line_reference() {
        let mut text = String::new();
        for i in 0..60_000 {
            let eol = if i % 7 == 0 { "\r\n" } else { "\n" };
            text.push_str(&format!("héllo {i} ñandú foo_{} bar  \t{eol}", i % 13));
        }
        let b = Buffer::from_text(&text);
        assert!(b.len_chars() > PAR_MIN_CHARS);
        let reference = |mt: &Matcher| -> Vec<Range<usize>> {
            let mut v = Vec::new();
            for i in 0..b.len_lines() {
                let base = b.line_start(i);
                v.extend(b.with_line(i, |l| mt.line_matches(l)).into_iter().map(|r| base + r.start..base + r.end));
            }
            v
        };
        let queries = [
            q("foo_7"),
            q("ñandú"),
            Query { regex: true, ..q(r"^héllo \d+") },
            Query { regex: true, ..q(r"bar\s+$") },
            Query { regex: true, ..q(r"bar\s+") }, // can run across a line break
            Query { whole_word: true, ..q("bar") },
            q("zzz-none"),
        ];
        for query in queries {
            let mt = m(query.clone());
            let want = reference(&mt);
            let got = find_all(&b, &mt, usize::MAX);
            let diff = got.iter().zip(&want).position(|(a, b)| a != b);
            assert!(got == want, "find_all {query:?}: {} vs {} hits, first diff at {diff:?}: {:?} vs {:?}", got.len(), want.len(), diff.map(|i| got[i].clone()), diff.map(|i| want[i].clone()));
            let from = b.len_chars() / 3;
            let next = want.iter().find(|r| r.start >= from).or(want.first()).cloned();
            assert_eq!(find_next(&b, &mt, from, true), next, "find_next {query:?}");
            let prev = want.iter().rev().find(|r| r.start < from).or(want.last()).cloned();
            assert_eq!(find_prev(&b, &mt, from, true), prev, "find_prev {query:?}");
            assert_eq!(find_all(&b, &mt, 5), want.iter().take(5).cloned().collect::<Vec<_>>());
        }
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
