//! Per-file git information for the editor: which lines differ from the index (gutter
//! markers), turning one change into a patch (stage a hunk), and `git blame` per line.

use std::ops::Range;
use std::path::Path;
use std::process::Command;

use similar::{Algorithm, DiffOp, TextDiff};

/// One changed region, in 0-based half-open *line* ranges of the old and new text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub old: Range<usize>,
    pub new: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    /// Lines were removed; `new` is empty and marks the spot.
    Deleted,
}

impl Change {
    pub fn kind(&self) -> ChangeKind {
        if self.old.is_empty() {
            ChangeKind::Added
        } else if self.new.is_empty() {
            ChangeKind::Deleted
        } else {
            ChangeKind::Modified
        }
    }

    /// Does the caret on `line` belong to this change? A deletion belongs to the line after it.
    pub fn contains_line(&self, line: usize) -> bool {
        if self.new.is_empty() { line == self.new.start } else { self.new.contains(&line) }
    }
}

/// The changes that turn `old` into `new`, in order.
pub fn diff_lines(old: &str, new: &str) -> Vec<Change> {
    TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .diff_lines(old, new)
        .ops()
        .iter()
        .filter_map(|op| match *op {
            DiffOp::Equal { .. } => None,
            DiffOp::Delete { old_index, old_len, new_index } => {
                Some(Change { old: old_index..old_index + old_len, new: new_index..new_index })
            }
            DiffOp::Insert { old_index, new_index, new_len } => {
                Some(Change { old: old_index..old_index, new: new_index..new_index + new_len })
            }
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                Some(Change { old: old_index..old_index + old_len, new: new_index..new_index + new_len })
            }
        })
        .collect()
}

fn lines_of(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// The old text of a change (to put back when reverting it).
pub fn old_text(old: &str, c: &Change) -> String {
    lines_of(old)[c.old.clone()].concat()
}

/// A zero-context unified diff of just this change, for `git apply --cached --unidiff-zero`.
pub fn hunk_patch(rel_path: &str, old: &str, new: &str, c: &Change) -> String {
    let (ol, nl) = (lines_of(old), lines_of(new));
    // With no lines on a side, git wants the line *before* the spot; otherwise the first line (1-based).
    let start = |r: &Range<usize>| if r.is_empty() { r.start } else { r.start + 1 };
    let mut p = format!(
        "--- a/{rel_path}\n+++ b/{rel_path}\n@@ -{},{} +{},{} @@\n",
        start(&c.old),
        c.old.len(),
        start(&c.new),
        c.new.len()
    );
    let mut body = |sign: char, lines: &[&str]| {
        for l in lines {
            p.push(sign);
            p.push_str(l);
            if !l.ends_with('\n') {
                p.push_str("\n\\ No newline at end of file\n");
            }
        }
    };
    body('-', &ol[c.old.clone()]);
    body('+', &nl[c.new.clone()]);
    p
}

/// The file as it is staged in the index (`None` for untracked or non-UTF-8 files).
pub fn index_text(repo_dir: &Path, file: &Path) -> Option<String> {
    let repo = git2::Repository::discover(repo_dir).ok()?;
    // Compare canonical paths: /var and /private/var are the same place on macOS.
    let (file, work) = (file.canonicalize().ok()?, repo.workdir()?.canonicalize().ok()?);
    let rel = file.strip_prefix(work).ok()?;
    let index = repo.index().ok()?;
    let entry = index.get_path(rel, 0)?;
    let blob = repo.find_blob(entry.id).ok()?;
    String::from_utf8(blob.content().to_vec()).ok()
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlameLine {
    pub author: String,
    /// Unix seconds.
    pub time: i64,
    pub summary: String,
    pub short_sha: String,
    pub committed: bool,
}

/// `git blame` of the file on disk, one entry per line.
pub fn blame(repo_dir: &Path, file: &Path) -> Result<Vec<BlameLine>, String> {
    let mut command = Command::new("git");
    crate::ops::apply_env(&mut command, repo_dir);
    let out = command
        .current_dir(repo_dir)
        .args(["blame", "--line-porcelain", "--"])
        .arg(file)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(parse_blame(&String::from_utf8_lossy(&out.stdout)))
}

pub fn parse_blame(text: &str) -> Vec<BlameLine> {
    let mut lines = Vec::new();
    let mut cur = BlameLine::default();
    for l in text.lines() {
        if let Some(code) = l.strip_prefix('\t') {
            let _ = code;
            lines.push(std::mem::take(&mut cur));
        } else if let Some(v) = l.strip_prefix("author ") {
            cur.author = v.to_owned();
        } else if let Some(v) = l.strip_prefix("author-time ") {
            cur.time = v.parse().unwrap_or(0);
        } else if let Some(v) = l.strip_prefix("summary ") {
            cur.summary = v.to_owned();
        } else if cur.short_sha.is_empty() {
            // The header line: "<40 hex sha> <orig line> <final line> [group size]".
            if let Some(sha) = l.split(' ').next().filter(|s| s.len() >= 40 && s.bytes().all(|b| b.is_ascii_hexdigit())) {
                cur.committed = !sha.bytes().all(|b| b == b'0');
                cur.short_sha = sha[..7].to_owned();
            }
        }
    }
    lines
}

/// "3 days ago" style, for the status bar.
pub fn ago(now: i64, then: i64) -> String {
    let s = (now - then).max(0);
    let (n, unit) = match s {
        0..=59 => return "just now".into(),
        60..=3599 => (s / 60, "minute"),
        3600..=86_399 => (s / 3600, "hour"),
        86_400..=2_591_999 => (s / 86_400, "day"),
        2_592_000..=31_535_999 => (s / 2_592_000, "month"),
        _ => (s / 31_536_000, "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_changes() {
        let old = "a\nb\nc\nd\n";
        let new = "a\nB\nc\nx\ny\n";
        let ch = diff_lines(old, new);
        assert_eq!(ch.len(), 2);
        assert_eq!((ch[0].kind(), ch[0].new.clone()), (ChangeKind::Modified, 1..2));
        assert_eq!(ch[1].old, 3..4);
        assert_eq!(ch[1].new, 3..5);
        let ch = diff_lines("a\nb\nc\n", "a\nc\n");
        assert_eq!(ch.len(), 1);
        assert_eq!(ch[0].kind(), ChangeKind::Deleted);
        assert!(ch[0].contains_line(1));
        assert!(!ch[0].contains_line(0));
        assert_eq!(diff_lines("same\n", "same\n"), vec![]);
        let ch = diff_lines("a\n", "a\nb\n");
        assert_eq!(ch[0].kind(), ChangeKind::Added);
    }

    #[test]
    fn hunk_patch_text() {
        let (old, new) = ("a\nb\nc\n", "a\nB\nc\n");
        let c = &diff_lines(old, new)[0];
        assert_eq!(hunk_patch("f.txt", old, new, c), "--- a/f.txt\n+++ b/f.txt\n@@ -2,1 +2,1 @@\n-b\n+B\n");
        // Pure insertion after line 1.
        let (old, new) = ("a\nc\n", "a\nb\nc\n");
        let c = &diff_lines(old, new)[0];
        assert_eq!(hunk_patch("f.txt", old, new, c), "--- a/f.txt\n+++ b/f.txt\n@@ -1,0 +2,1 @@\n+b\n");
        // Missing final newline is spelled out.
        let (old, new) = ("a\nb", "a\nB");
        let c = &diff_lines(old, new)[0];
        assert!(hunk_patch("f.txt", old, new, c).contains("\\ No newline at end of file"));
    }

    #[test]
    fn old_text_of_a_change() {
        let (old, new) = ("a\nb\nc\n", "a\nX\nY\nc\n");
        let c = &diff_lines(old, new)[0];
        assert_eq!(old_text(old, c), "b\n");
    }

    #[test]
    fn parses_blame_porcelain() {
        let sha = "a".repeat(40);
        let zero = "0".repeat(40);
        let text = format!(
            "{sha} 1 1 1\nauthor Ada\nauthor-time 1700000000\nsummary First\nfilename f\n\tline one\n{zero} 2 2 1\nauthor Not Committed Yet\nauthor-time 1700000100\nsummary Version of f\n\tline two\n"
        );
        let b = parse_blame(&text);
        assert_eq!(b.len(), 2);
        assert_eq!((b[0].author.as_str(), b[0].short_sha.as_str(), b[0].committed), ("Ada", "aaaaaaa", true));
        assert!(!b[1].committed);
    }

    #[test]
    fn registered_environment_reaches_git() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for a in [["init", "-q"].as_slice(), ["config", "commit.gpgsign", "false"].as_slice()] {
            assert!(Command::new("git").current_dir(&root).args(a).status().unwrap().success());
        }
        std::fs::write(root.join("f"), "x").unwrap();
        crate::ops::stage(&root, &["f".to_owned()]).unwrap();
        // No identity configured anywhere except the profile environment.
        crate::ops::set_env(
            &root,
            crate::ops::GitEnv {
                set: vec![
                    ("GIT_AUTHOR_NAME".into(), "Work Me".into()),
                    ("GIT_AUTHOR_EMAIL".into(), "me@work.example".into()),
                    ("GIT_COMMITTER_NAME".into(), "Work Me".into()),
                    ("GIT_COMMITTER_EMAIL".into(), "me@work.example".into()),
                    ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
                ],
                unset: vec![],
            },
        );
        crate::ops::commit(&root, "as work", false).unwrap();
        let log = Command::new("git").current_dir(&root).args(["log", "-1", "--format=%an <%ae>"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "Work Me <me@work.example>");
    }

    #[test]
    fn relative_times() {
        assert_eq!(ago(1000, 990), "just now");
        assert_eq!(ago(10_000, 10_000 - 7200), "2 hours ago");
        assert_eq!(ago(1_000_000, 1_000_000 - 86_400), "1 day ago");
        assert_eq!(ago(100_000_000, 0), "3 years ago");
    }

    #[test]
    fn stage_a_hunk_for_real() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let o = Command::new("git").current_dir(dir.path()).args(args).output().unwrap();
            assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
            String::from_utf8_lossy(&o.stdout).into_owned()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "T"]);
        let base = "1\n2\n3\n4\n5\n6\n7\n8\n";
        std::fs::write(dir.path().join("f.txt"), base).unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "base"]);
        // Two separate edits in the working copy: line 2 and line 7.
        let edited = "1\nTWO\n3\n4\n5\n6\nSEVEN\n8\n";
        std::fs::write(dir.path().join("f.txt"), edited).unwrap();

        let staged = index_text(dir.path(), &dir.path().join("f.txt")).unwrap();
        assert_eq!(staged, base);
        let changes = diff_lines(&staged, edited);
        assert_eq!(changes.len(), 2);
        // Stage only the second hunk.
        let patch = hunk_patch("f.txt", &staged, edited, &changes[1]);
        crate::ops::stage_patch(dir.path(), &patch).unwrap();
        let cached = git(&["diff", "--cached", "--unified=0"]);
        assert!(cached.contains("+SEVEN") && !cached.contains("+TWO"), "{cached}");
        // The first hunk is still unstaged.
        let after = index_text(dir.path(), &dir.path().join("f.txt")).unwrap();
        assert_eq!(diff_lines(&after, edited).len(), 1);

        // Blame sees the commit and the uncommitted lines.
        let b = blame(dir.path(), &dir.path().join("f.txt")).unwrap();
        assert_eq!(b.len(), 8);
        assert!(b[0].committed && b[0].author == "T" && b[0].summary == "base");
        assert!(!b[1].committed);
    }
}
