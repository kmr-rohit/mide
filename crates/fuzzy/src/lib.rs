//! Fuzzy matching for Cmd+P (files), Cmd+Shift+P (commands) and friends.
//!
//! Thin layer over `nucleo-matcher` that returns ranked results together with
//! the matched character positions, so the UI can highlight them.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// Index into the slice of candidates that was searched.
    pub index: usize,
    pub score: u32,
    /// Char (not byte) positions in the candidate that matched, ascending.
    pub indices: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Plain text such as command names.
    Text,
    /// File paths: separators are word boundaries, and the file name is
    /// weighted over directories.
    Path,
}

pub struct Fuzzy {
    matcher: Matcher,
    buf: Vec<char>,
}

impl Fuzzy {
    pub fn new(kind: Kind) -> Self {
        let config = match kind {
            Kind::Text => Config::DEFAULT,
            Kind::Path => Config::DEFAULT.match_paths(),
        };
        Self { matcher: Matcher::new(config), buf: Vec::new() }
    }

    /// Rank `items` against `query`. An empty query returns the first `limit`
    /// items in their original order. Ties are broken by shorter candidate, then
    /// original order, so results are stable.
    pub fn search<S: AsRef<str>>(&mut self, items: &[S], query: &str, limit: usize) -> Vec<Match> {
        self.search_boosted(items, query, limit, |_| 0)
    }

    /// Like [`search`](Self::search) with a per-item score bonus, e.g. for
    /// recently opened files.
    pub fn search_boosted<S: AsRef<str>>(
        &mut self,
        items: &[S],
        query: &str,
        limit: usize,
        boost: impl Fn(usize) -> u32,
    ) -> Vec<Match> {
        if query.trim().is_empty() {
            return (0..items.len().min(limit))
                .map(|index| Match { index, score: 0, indices: Vec::new() })
                .collect();
        }
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let mut out: Vec<Match> = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let hay = Utf32Str::new(item.as_ref(), &mut self.buf);
            if let Some(score) = pattern.score(hay, &mut self.matcher) {
                out.push(Match { index, score: score.saturating_add(boost(index)), indices: Vec::new() });
            }
        }
        out.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| items[a.index].as_ref().len().cmp(&items[b.index].as_ref().len()))
                .then_with(|| a.index.cmp(&b.index))
        });
        out.truncate(limit);
        // Positions are only computed for the rows we'll actually show.
        for m in &mut out {
            let hay = Utf32Str::new(items[m.index].as_ref(), &mut self.buf);
            let mut idx = Vec::new();
            pattern.indices(hay, &mut self.matcher, &mut idx);
            idx.sort_unstable();
            idx.dedup();
            m.indices = idx;
        }
        out
    }
}

/// Split `text` into `(segment, highlighted)` runs for rendering, given char
/// indices from a [`Match`].
pub fn highlight_runs(text: &str, indices: &[u32]) -> Vec<(String, bool)> {
    let mut runs: Vec<(String, bool)> = Vec::new();
    let mut next = indices.iter().copied().peekable();
    for (i, ch) in text.chars().enumerate() {
        while next.peek().is_some_and(|&n| (n as usize) < i) {
            next.next();
        }
        let hit = next.peek() == Some(&(i as u32));
        match runs.last_mut() {
            Some((s, h)) if *h == hit => s.push(ch),
            _ => runs.push((ch.to_string(), hit)),
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names<S: AsRef<str>>(items: &[S], ms: &[Match]) -> Vec<String> {
        ms.iter().map(|m| items[m.index].as_ref().to_owned()).collect()
    }

    #[test]
    fn ranks_filename_over_directory_for_paths() {
        let files = ["src/editor/mod.rs", "src/main.rs", "docs/maintenance.md", "tests/main_test.rs"];
        let mut f = Fuzzy::new(Kind::Path);
        let r = f.search(&files, "main", 10);
        assert_eq!(names(&files, &r)[0], "src/main.rs");
        assert!(!names(&files, &r).contains(&"src/editor/mod.rs".to_owned()));
    }

    #[test]
    fn subsequence_and_case() {
        let cmds = ["Build: New Build System", "Bookmarks: Toggle", "Code Folding: Fold All", "Convert Case: Lower Case"];
        let mut f = Fuzzy::new(Kind::Text);
        assert_eq!(names(&cmds, &f.search(&cmds, "cclc", 10)), vec!["Convert Case: Lower Case"]);
        // Smart case: lowercase query matches any case; uppercase is strict.
        assert_eq!(f.search(&cmds, "fold", 10).len(), 1);
        assert!(f.search(&cmds, "FOLD", 10).is_empty());
    }

    #[test]
    fn indices_point_at_matched_chars() {
        let items = ["foo_bar.rs"];
        let mut f = Fuzzy::new(Kind::Path);
        let r = f.search(&items, "fbr", 5);
        assert_eq!(r[0].indices, vec![0, 4, 8]);
        let runs = highlight_runs("foo_bar.rs", &r[0].indices);
        let strs: Vec<(&str, bool)> = runs.iter().map(|(s, h)| (s.as_str(), *h)).collect();
        assert_eq!(strs, vec![("f", true), ("oo_", false), ("b", true), ("ar.", false), ("r", true), ("s", false)]);
    }

    #[test]
    fn empty_query_keeps_order_and_limit() {
        let items = ["b", "a", "c"];
        let mut f = Fuzzy::new(Kind::Text);
        let r = f.search(&items, "  ", 2);
        assert_eq!(r.iter().map(|m| m.index).collect::<Vec<_>>(), vec![0, 1]);
    }

    #[test]
    fn boost_reorders() {
        let items = ["alpha_one", "alpha_two"];
        let mut f = Fuzzy::new(Kind::Text);
        let plain = f.search(&items, "alpha", 5);
        let boosted = f.search_boosted(&items, "alpha", 5, |i| if i == 1 { 1000 } else { 0 });
        assert_eq!(plain[0].index, 0);
        assert_eq!(boosted[0].index, 1);
    }

    #[test]
    fn unicode_indices_are_char_based() {
        let items = ["naïve_café.txt"];
        let mut f = Fuzzy::new(Kind::Text);
        let r = f.search(&items, "cafe", 5);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].indices.len(), 4);
        assert!(r[0].indices.iter().all(|&i| (i as usize) < "naïve_café.txt".chars().count()));
    }
}
