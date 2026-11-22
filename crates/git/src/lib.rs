//! Git data layer for the IDE: status bar info, refs sidebar, commit log with
//! graph lanes, and commit diffs. No UI code here.

pub mod diff;
pub mod graph;
pub mod ops;

use std::collections::HashMap;
use std::path::Path;

use git2::{DiffOptions, ErrorCode, Patch, Repository, Sort, StatusOptions};

pub use diff::{FileDiff, FileStatus, Hunk, LineKind};
pub use git2::Oid;
pub use graph::{GraphBuilder, GraphRow};

pub type Result<T> = std::result::Result<T, git2::Error>;

// ---- public types ------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    LocalBranch,
    RemoteBranch,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefLabel {
    pub name: String,
    pub kind: RefKind,
    /// True for the branch HEAD points at.
    pub is_head: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Refs {
    pub branches: Vec<String>,
    pub remotes: Vec<String>,
    pub tags: Vec<String>,
    pub stashes: Vec<String>,
    pub submodules: Vec<String>,
    pub worktrees: Vec<String>,
}

/// Everything the status bar shows: `Git branch: master, index: ✓, working: ✓`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSummary {
    /// Branch name, or short hash when detached, or `None` for an unborn branch
    /// with no commits.
    pub branch: Option<String>,
    pub detached: bool,
    pub index_clean: bool,
    pub working_clean: bool,
    pub ahead: usize,
    pub behind: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeState {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChange,
    Untracked,
    Conflicted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: String,
    pub index: Option<ChangeState>,
    pub worktree: Option<ChangeState>,
}

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub id: Oid,
    pub summary: String,
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    /// Unix seconds.
    pub time: i64,
    pub parents: Vec<Oid>,
    pub labels: Vec<RefLabel>,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub commit: CommitInfo,
    pub graph: GraphRow,
}

#[derive(Debug, Clone)]
pub struct CommitDetail {
    pub info: CommitInfo,
    pub tree: Oid,
    pub files: Vec<FileDiff>,
    pub additions: usize,
    pub deletions: usize,
}

// ---- repository ----------------------------------------------------------------

pub struct Repo {
    inner: Repository,
}

impl Repo {
    /// Open the repository containing `path` (walks up like `git` does).
    pub fn discover(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self { inner: Repository::discover(path)? })
    }

    pub fn workdir(&self) -> Option<&Path> {
        self.inner.workdir()
    }

    pub fn status_summary(&self) -> Result<StatusSummary> {
        let (branch, detached) = match self.inner.head() {
            Ok(head) if head.is_branch() => (head.shorthand().ok().map(str::to_owned), false),
            Ok(head) => (
                head.target().map(short),
                true,
            ),
            Err(e) if e.code() == ErrorCode::UnbornBranch => {
                // Name of the branch-to-be, from HEAD's symbolic target.
                let name = self
                    .inner
                    .find_reference("HEAD")
                    .ok()
                    .and_then(|r| r.symbolic_target().ok().flatten().map(str::to_owned))
                    .map(|t| t.trim_start_matches("refs/heads/").to_owned());
                (name, false)
            }
            Err(e) => return Err(e),
        };

        let mut index_clean = true;
        let mut working_clean = true;
        for e in self.status()? {
            index_clean &= e.index.is_none();
            working_clean &= e.worktree.is_none();
        }

        let (ahead, behind) = self.ahead_behind().unwrap_or((0, 0));
        Ok(StatusSummary { branch, detached, index_clean, working_clean, ahead, behind })
    }

    fn ahead_behind(&self) -> Result<(usize, usize)> {
        let head = self.inner.head()?;
        let local = head.target().ok_or_else(|| git2::Error::from_str("no head target"))?;
        let branch = git2::Branch::wrap(head);
        let upstream = branch.upstream()?;
        let up = upstream.get().target().ok_or_else(|| git2::Error::from_str("no upstream target"))?;
        self.inner.graph_ahead_behind(local, up)
    }

    pub fn status(&self) -> Result<Vec<StatusEntry>> {
        use git2::Status as S;
        let mut opts = StatusOptions::new();
        opts.include_untracked(true).recurse_untracked_dirs(true).renames_head_to_index(true);
        let statuses = self.inner.statuses(Some(&mut opts))?;
        let mut out = Vec::new();
        for s in statuses.iter() {
            let f = s.status();
            if f == S::CURRENT || f.contains(S::IGNORED) {
                continue;
            }
            let index = if f.contains(S::INDEX_NEW) {
                Some(ChangeState::Added)
            } else if f.contains(S::INDEX_MODIFIED) {
                Some(ChangeState::Modified)
            } else if f.contains(S::INDEX_DELETED) {
                Some(ChangeState::Deleted)
            } else if f.contains(S::INDEX_RENAMED) {
                Some(ChangeState::Renamed)
            } else if f.contains(S::INDEX_TYPECHANGE) {
                Some(ChangeState::TypeChange)
            } else {
                None
            };
            let worktree = if f.contains(S::CONFLICTED) {
                Some(ChangeState::Conflicted)
            } else if f.contains(S::WT_NEW) {
                Some(ChangeState::Untracked)
            } else if f.contains(S::WT_MODIFIED) {
                Some(ChangeState::Modified)
            } else if f.contains(S::WT_DELETED) {
                Some(ChangeState::Deleted)
            } else if f.contains(S::WT_RENAMED) {
                Some(ChangeState::Renamed)
            } else if f.contains(S::WT_TYPECHANGE) {
                Some(ChangeState::TypeChange)
            } else {
                None
            };
            out.push(StatusEntry {
                path: s.path().unwrap_or("").to_owned(),
                index,
                worktree,
            });
        }
        Ok(out)
    }

    /// Contents of the "Locations" sidebar.
    pub fn refs(&mut self) -> Result<Refs> {
        let mut refs = Refs::default();
        for r in self.inner.references()? {
            let r = r?;
            let Ok(name) = r.name() else { continue };
            if let Some(n) = name.strip_prefix("refs/heads/") {
                refs.branches.push(n.to_owned());
            } else if let Some(n) = name.strip_prefix("refs/remotes/") {
                if !n.ends_with("/HEAD") {
                    refs.remotes.push(n.to_owned());
                }
            } else if let Some(n) = name.strip_prefix("refs/tags/") {
                refs.tags.push(n.to_owned());
            }
        }
        refs.branches.sort();
        refs.remotes.sort();
        refs.tags.sort();
        refs.submodules = self
            .inner
            .submodules()?
            .iter()
            .filter_map(|s| s.path().to_str().map(str::to_owned))
            .collect();
        refs.worktrees = self
            .inner
            .worktrees()?
            .iter()
            .filter_map(|w| w.ok().flatten().map(str::to_owned))
            .collect();
        let mut stashes = Vec::new();
        self.inner.stash_foreach(|_, msg, _| {
            stashes.push(msg.to_owned());
            true
        })?;
        refs.stashes = stashes;
        Ok(refs)
    }

    fn label_map(&self) -> Result<HashMap<Oid, Vec<RefLabel>>> {
        let head_name = self.inner.head().ok().and_then(|h| {
            h.is_branch().then(|| h.name().ok().map(str::to_owned)).flatten()
        });
        let mut map: HashMap<Oid, Vec<RefLabel>> = HashMap::new();
        for r in self.inner.references()? {
            let r = r?;
            let Ok(full) = r.name().map(str::to_owned) else { continue };
            let (kind, short_name) = if let Some(n) = full.strip_prefix("refs/heads/") {
                (RefKind::LocalBranch, n)
            } else if let Some(n) = full.strip_prefix("refs/remotes/") {
                if n.ends_with("/HEAD") {
                    continue;
                }
                (RefKind::RemoteBranch, n)
            } else if let Some(n) = full.strip_prefix("refs/tags/") {
                (RefKind::Tag, n)
            } else {
                continue;
            };
            let Ok(commit) = r.peel_to_commit() else { continue };
            map.entry(commit.id()).or_default().push(RefLabel {
                name: short_name.to_owned(),
                kind,
                is_head: head_name.as_deref() == Some(full.as_str()),
            });
        }
        for labels in map.values_mut() {
            // Tags, then local branches (HEAD first), then remotes.
            labels.sort_by_key(|l| {
                (
                    match l.kind {
                        RefKind::Tag => 0,
                        RefKind::LocalBranch => 1,
                        RefKind::RemoteBranch => 2,
                    },
                    !l.is_head,
                    l.name.clone(),
                )
            });
        }
        Ok(map)
    }

    /// Commit log across all branches (like `git log --all --topo-order`) with
    /// graph lanes. `skip`/`limit` allow paging as the user scrolls.
    pub fn log(&self, skip: usize, limit: usize) -> Result<Vec<LogEntry>> {
        let labels = self.label_map()?;
        let mut walk = self.inner.revwalk()?;
        walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
        let mut pushed = false;
        for r in self.inner.references()? {
            let r = r?;
            let Ok(name) = r.name() else { continue };
            if name.starts_with("refs/heads/") || name.starts_with("refs/remotes/") || name.starts_with("refs/tags/") {
                if let Ok(c) = r.peel_to_commit() {
                    walk.push(c.id())?;
                    pushed = true;
                }
            }
        }
        if !pushed {
            return Ok(Vec::new());
        }

        // Lanes depend on every earlier row, so we run the builder from the top
        // and only materialize the requested window.
        let mut builder = GraphBuilder::new();
        let mut out = Vec::with_capacity(limit);
        for (i, oid) in walk.enumerate() {
            if i >= skip + limit {
                break;
            }
            let commit = self.inner.find_commit(oid?)?;
            let parents: Vec<Oid> = commit.parent_ids().collect();
            let graph = builder.push(commit.id(), &parents);
            if i >= skip {
                out.push(LogEntry {
                    commit: commit_info(&commit, labels.get(&commit.id()).cloned().unwrap_or_default()),
                    graph,
                });
            }
        }
        Ok(out)
    }

    /// Full detail for the right-hand pane: metadata plus per-file diffs against
    /// the first parent (or the empty tree for a root commit).
    pub fn commit_detail(&self, id: Oid) -> Result<CommitDetail> {
        let commit = self.inner.find_commit(id)?;
        let labels = self.label_map()?;
        let tree = commit.tree()?;
        let parent_tree = if commit.parent_count() > 0 { Some(commit.parent(0)?.tree()?) } else { None };
        let files = self.diff_trees(parent_tree.as_ref(), Some(&tree))?;
        let additions = files.iter().map(|f| f.additions).sum();
        let deletions = files.iter().map(|f| f.deletions).sum();
        Ok(CommitDetail {
            info: commit_info(&commit, labels.get(&id).cloned().unwrap_or_default()),
            tree: tree.id(),
            files,
            additions,
            deletions,
        })
    }

    /// Diff for the working tree vs the index (`staged = false`) or the index
    /// vs HEAD (`staged = true`).
    pub fn working_diff(&self, staged: bool) -> Result<Vec<FileDiff>> {
        let mut opts = DiffOptions::new();
        opts.include_untracked(!staged).recurse_untracked_dirs(true).show_untracked_content(true);
        let diff = if staged {
            let head_tree = self.inner.head().ok().and_then(|h| h.peel_to_tree().ok());
            self.inner.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut opts))?
        } else {
            self.inner.diff_index_to_workdir(None, Some(&mut opts))?
        };
        collect_files(diff)
    }

    fn diff_trees(&self, old: Option<&git2::Tree>, new: Option<&git2::Tree>) -> Result<Vec<FileDiff>> {
        let mut opts = DiffOptions::new();
        let diff = self.inner.diff_tree_to_tree(old, new, Some(&mut opts))?;
        collect_files(diff)
    }
}

fn short(id: Oid) -> String {
    id.to_string()[..7].to_owned()
}

fn commit_info(c: &git2::Commit, labels: Vec<RefLabel>) -> CommitInfo {
    let author = c.author();
    CommitInfo {
        id: c.id(),
        summary: c.summary().ok().flatten().unwrap_or("").to_owned(),
        message: c.message().unwrap_or("").to_owned(),
        author_name: author.name().unwrap_or("").to_owned(),
        author_email: author.email().unwrap_or("").to_owned(),
        time: c.time().seconds(),
        parents: c.parent_ids().collect(),
        labels,
    }
}

fn collect_files(mut diff: git2::Diff) -> Result<Vec<FileDiff>> {
    let mut find = git2::DiffFindOptions::new();
    find.renames(true);
    diff.find_similar(Some(&mut find))?;

    let mut files = Vec::new();
    for i in 0..diff.deltas().len() {
        let Some(patch) = Patch::from_diff(&diff, i)? else { continue }; // binary / unchanged
        let delta = patch.delta();
        let (_, additions, deletions) = patch.line_stats()?;
        let mut hunks = Vec::new();
        for h in 0..patch.num_hunks() {
            let (hunk, n_lines) = patch.hunk(h)?;
            let mut lines = Vec::with_capacity(n_lines);
            for l in 0..n_lines {
                let line = patch.line_in_hunk(h, l)?;
                let kind = match line.origin() {
                    '+' => LineKind::Added,
                    '-' => LineKind::Removed,
                    ' ' => LineKind::Context,
                    _ => continue, // "\ No newline at end of file" markers etc.
                };
                let text = String::from_utf8_lossy(line.content());
                lines.push(diff::DiffLine {
                    kind,
                    old_lineno: line.old_lineno(),
                    new_lineno: line.new_lineno(),
                    text: text.trim_end_matches(['\n', '\r']).to_owned(),
                });
            }
            hunks.push(Hunk {
                header: String::from_utf8_lossy(hunk.header()).trim_end().to_owned(),
                old_start: hunk.old_start(),
                new_start: hunk.new_start(),
                lines,
            });
        }
        files.push(FileDiff {
            old_path: delta.old_file().path().map(|p| p.to_string_lossy().into_owned()),
            new_path: delta.new_file().path().map(|p| p.to_string_lossy().into_owned()),
            status: match delta.status() {
                git2::Delta::Added | git2::Delta::Untracked => FileStatus::Added,
                git2::Delta::Deleted => FileStatus::Deleted,
                git2::Delta::Modified => FileStatus::Modified,
                git2::Delta::Renamed => FileStatus::Renamed,
                git2::Delta::Copied => FileStatus::Copied,
                git2::Delta::Typechange => FileStatus::TypeChange,
                _ => FileStatus::Other,
            },
            binary: delta.flags().is_binary(),
            additions,
            deletions,
            hunks,
        });
    }
    Ok(files)
}

#[cfg(test)]
mod tests;
