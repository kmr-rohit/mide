//! File/hunk/line diff model plus the side-by-side view used by the diff pane.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
    /// Without the trailing newline.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub header: String,
    pub old_start: u32,
    pub new_start: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
    TypeChange,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub status: FileStatus,
    pub binary: bool,
    pub additions: usize,
    pub deletions: usize,
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    /// Path to show in the UI (new path, falling back to old for deletions).
    pub fn display_path(&self) -> &str {
        self.new_path.as_deref().or(self.old_path.as_deref()).unwrap_or("")
    }
}

// ---- side-by-side ----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub lineno: u32,
    pub text: String,
    pub kind: LineKind,
    /// Char range inside `text` that actually differs from the paired line
    /// (for intra-line highlighting). `None` for context / unpaired lines.
    pub changed: Option<Range<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SideBySideRow {
    pub left: Option<Cell>,
    pub right: Option<Cell>,
}

/// Pair removed/added runs index-wise into left/right columns.
pub fn side_by_side(hunk: &Hunk) -> Vec<SideBySideRow> {
    let mut rows = Vec::with_capacity(hunk.lines.len());
    let mut removed: Vec<&DiffLine> = Vec::new();
    let mut added: Vec<&DiffLine> = Vec::new();

    fn flush(rows: &mut Vec<SideBySideRow>, removed: &mut Vec<&DiffLine>, added: &mut Vec<&DiffLine>) {
        let n = removed.len().max(added.len());
        for i in 0..n {
            let l = removed.get(i).copied();
            let r = added.get(i).copied();
            let (lc, rc) = match (l, r) {
                (Some(l), Some(r)) => {
                    let (lr, rr) = changed_ranges(&l.text, &r.text);
                    (Some((l, Some(lr))), Some((r, Some(rr))))
                }
                (l, r) => (l.map(|l| (l, None)), r.map(|r| (r, None))),
            };
            rows.push(SideBySideRow {
                left: lc.map(|(l, ch)| Cell {
                    lineno: l.old_lineno.unwrap_or(0),
                    text: l.text.clone(),
                    kind: LineKind::Removed,
                    changed: ch,
                }),
                right: rc.map(|(r, ch)| Cell {
                    lineno: r.new_lineno.unwrap_or(0),
                    text: r.text.clone(),
                    kind: LineKind::Added,
                    changed: ch,
                }),
            });
        }
        removed.clear();
        added.clear();
    }

    for line in &hunk.lines {
        match line.kind {
            LineKind::Removed => removed.push(line),
            LineKind::Added => added.push(line),
            LineKind::Context => {
                flush(&mut rows, &mut removed, &mut added);
                rows.push(SideBySideRow {
                    left: Some(Cell {
                        lineno: line.old_lineno.unwrap_or(0),
                        text: line.text.clone(),
                        kind: LineKind::Context,
                        changed: None,
                    }),
                    right: Some(Cell {
                        lineno: line.new_lineno.unwrap_or(0),
                        text: line.text.clone(),
                        kind: LineKind::Context,
                        changed: None,
                    }),
                });
            }
        }
    }
    flush(&mut rows, &mut removed, &mut added);
    rows
}

/// Differing char ranges after trimming the common prefix and suffix.
pub fn changed_ranges(a: &str, b: &str) -> (Range<usize>, Range<usize>) {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let max_suffix = a.len().min(b.len()) - prefix;
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(max_suffix)
        .take_while(|(x, y)| x == y)
        .count();
    (prefix..a.len() - suffix, prefix..b.len() - suffix)
}
