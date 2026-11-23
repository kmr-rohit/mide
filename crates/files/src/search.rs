//! Find in files: a regex/plain search over every text file under a folder, honoring
//! `.gitignore`. Runs on the caller's thread and reports hits in batches, so the UI can
//! show results while it is still working and stop it any time.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use ignore::WalkBuilder;
use mide_core::search::{Matcher, Query};

/// Files larger than this are skipped (generated bundles, data dumps).
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// A single hit's line text is cut to this many chars.
const MAX_TEXT_CHARS: usize = 200;
/// Hits taken from one file, so one giant file cannot fill the list.
const MAX_PER_FILE: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    /// `/`-separated path relative to the searched folder.
    pub rel: String,
    /// 0-based line and char column of the match in the file.
    pub line: usize,
    pub col: usize,
    /// The line with leading whitespace removed and cut to a sensible length.
    pub text: String,
    /// The match inside `text`, as char offsets.
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub files_with_hits: usize,
    pub hits: usize,
    /// Stopped at the global limit: there are more matches than listed.
    pub truncated: bool,
}

/// Search `root`. `on_batch` is called once per file that has hits. Returns early when
/// `cancel` is set. `Err` is a readable message for an invalid pattern.
pub fn search(
    root: &Path,
    q: &Query,
    limit: usize,
    cancel: &AtomicBool,
    mut on_batch: impl FnMut(Vec<Hit>),
) -> Result<Summary, String> {
    let Some(matcher) = Matcher::new(q)? else { return Ok(Summary::default()) };
    let mut summary = Summary::default();
    let walk = WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            n != ".git" && n != ".DS_Store"
        })
        .build();
    for entry in walk.flatten() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if entry.metadata().map_or(true, |m| m.len() > MAX_FILE_BYTES) {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else { continue };
        if bytes.iter().take(8192).any(|&b| b == 0) {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else { continue };
        let rel = path
            .strip_prefix(root)
            .map(|r| r.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"))
            .unwrap_or_default();
        let mut hits = Vec::new();
        'lines: for (line_no, line) in text.lines().enumerate() {
            for m in matcher.line_matches(line) {
                let lead = line.chars().take_while(|c| c.is_whitespace()).count();
                let shown: String = line.chars().skip(lead).take(MAX_TEXT_CHARS).collect();
                let (start, end) = (m.start.saturating_sub(lead), m.end.saturating_sub(lead));
                hits.push(Hit {
                    path: path.to_owned(),
                    rel: rel.clone(),
                    line: line_no,
                    col: m.start,
                    start: start.min(MAX_TEXT_CHARS),
                    end: end.min(MAX_TEXT_CHARS),
                    text: shown,
                });
                if hits.len() >= MAX_PER_FILE || summary.hits + hits.len() >= limit {
                    break 'lines;
                }
            }
        }
        if hits.is_empty() {
            continue;
        }
        summary.files_with_hits += 1;
        summary.hits += hits.len();
        on_batch(hits);
        if summary.hits >= limit {
            summary.truncated = true;
            break;
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn project() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        fs::create_dir_all(r.join("src")).unwrap();
        fs::create_dir_all(r.join("target")).unwrap();
        fs::write(r.join(".gitignore"), "target/\n").unwrap();
        fs::write(r.join("src/a.rs"), "fn main() {\n    let needle = 1;\n}\n// Needle again\n").unwrap();
        fs::write(r.join("src/b.txt"), "nothing here\n").unwrap();
        fs::write(r.join("target/out.rs"), "needle in ignored\n").unwrap();
        fs::write(r.join("bin.dat"), b"needle\0\0\0").unwrap();
        d
    }

    fn run(root: &Path, q: Query) -> (Vec<Hit>, Result<Summary, String>) {
        let mut all = Vec::new();
        let r = search(root, &q, 1000, &AtomicBool::new(false), |h| all.extend(h));
        (all, r)
    }

    #[test]
    fn finds_hits_skipping_ignored_and_binary_files() {
        let d = project();
        let (hits, sum) = run(d.path(), Query { text: "needle".into(), ..Default::default() });
        let rels: Vec<_> = hits.iter().map(|h| (h.rel.as_str(), h.line)).collect();
        assert_eq!(rels, vec![("src/a.rs", 1), ("src/a.rs", 3)]);
        assert_eq!(sum.unwrap(), Summary { files_with_hits: 1, hits: 2, truncated: false });
    }

    #[test]
    fn hit_positions_and_trimmed_text() {
        let d = project();
        let (hits, _) = run(d.path(), Query { text: "needle".into(), case_sensitive: true, ..Default::default() });
        assert_eq!(hits.len(), 1);
        let h = &hits[0];
        assert_eq!((h.line, h.col), (1, 8));
        assert_eq!(h.text, "let needle = 1;");
        assert_eq!(&h.text[h.start..h.end], "needle");
    }

    #[test]
    fn regex_and_invalid_pattern() {
        let d = project();
        let (hits, _) = run(d.path(), Query { text: r"fn\s+\w+".into(), regex: true, ..Default::default() });
        assert_eq!(hits.len(), 1);
        let (_, r) = run(d.path(), Query { text: "(".into(), regex: true, ..Default::default() });
        assert!(r.is_err());
    }

    #[test]
    fn limit_and_cancel() {
        let d = project();
        let mut n = 0;
        let s = search(d.path(), &Query { text: "e".into(), ..Default::default() }, 3, &AtomicBool::new(false), |h| {
            n += h.len()
        })
        .unwrap();
        assert!(s.truncated);
        assert_eq!(n, 3);
        let mut n = 0;
        search(d.path(), &Query { text: "e".into(), ..Default::default() }, 100, &AtomicBool::new(true), |h| n += h.len())
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn empty_query_finds_nothing() {
        let d = project();
        let (hits, sum) = run(d.path(), Query::default());
        assert!(hits.is_empty());
        assert_eq!(sum.unwrap(), Summary::default());
    }
}
