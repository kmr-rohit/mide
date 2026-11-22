use super::*;
use crate::diff::{changed_ranges, side_by_side, Cell};
use git2::{Repository, RepositoryInitOptions, Signature, Time, Tree};
use tempfile::TempDir;

fn init() -> (TempDir, Repository) {
    let dir = tempfile::tempdir().unwrap();
    let mut opts = RepositoryInitOptions::new();
    opts.initial_head("master");
    let repo = Repository::init_opts(dir.path(), &opts).unwrap();
    (dir, repo)
}

fn sig(t: i64) -> Signature<'static> {
    Signature::new("Tester", "t@example.com", &Time::new(1_700_000_000 + t, 0)).unwrap()
}

/// Commit a top-level-files-only tree built on `base`.
fn commit_tree(
    repo: &Repository,
    base: Option<&Tree>,
    files: &[(&str, &str)],
    msg: &str,
    parents: &[Oid],
    update_ref: &str,
    t: i64,
) -> Oid {
    let mut tb = repo.treebuilder(base).unwrap();
    for (name, content) in files {
        let blob = repo.blob(content.as_bytes()).unwrap();
        tb.insert(name, blob, 0o100644).unwrap();
    }
    let tree = repo.find_tree(tb.write().unwrap()).unwrap();
    let parents: Vec<_> = parents.iter().map(|p| repo.find_commit(*p).unwrap()).collect();
    let parent_refs: Vec<_> = parents.iter().collect();
    repo.commit(Some(update_ref), &sig(t), &sig(t), msg, &tree, &parent_refs).unwrap()
}

/// c1 --- c3 --- c4 (master, merge)
///   \          /
///    `-- c2 --'   (feature)
fn diamond() -> (TempDir, Repo, [Oid; 4]) {
    let (dir, raw) = init();
    let ids = {
        let repo = &raw;
        let c1 = commit_tree(repo, None, &[("a.txt", "one\ntwo\nthree\n")], "c1", &[], "HEAD", 1);
        let t1 = repo.find_commit(c1).unwrap().tree().unwrap();
        let c2 = commit_tree(repo, Some(&t1), &[("b.txt", "bee\n")], "c2", &[c1], "refs/heads/feature", 2);
        let c3 = commit_tree(repo, Some(&t1), &[("a.txt", "one\nTWO\nthree\n")], "c3", &[c1], "HEAD", 3);
        let t3 = repo.find_commit(c3).unwrap().tree().unwrap();
        let c4 = commit_tree(repo, Some(&t3), &[("b.txt", "bee\n")], "merge", &[c3, c2], "HEAD", 4);
        [c1, c2, c3, c4]
    };
    drop(raw);
    let repo = Repo::discover(dir.path()).unwrap();
    (dir, repo, ids)
}

#[test]
fn log_has_graph_lanes_for_a_merge() {
    let (_d, repo, [c1, c2, c3, c4]) = diamond();
    let log = repo.log(0, 100).unwrap();
    assert_eq!(log.len(), 4);

    // Newest first: the merge commit, in lane 0, forks into two lanes.
    assert_eq!(log[0].commit.id, c4);
    assert_eq!(log[0].graph.column, 0);
    assert!(log[0].graph.outgoing.contains(&graph::Edge { from: 0, to: 0 }));
    assert!(log[0].graph.outgoing.contains(&graph::Edge { from: 0, to: 1 }));

    // c2 and c3 appear in the middle in some order, in different lanes.
    let mid: Vec<_> = log[1..3].iter().map(|e| (e.commit.id, e.graph.column)).collect();
    assert!(mid.iter().any(|(id, _)| *id == c2));
    assert!(mid.iter().any(|(id, _)| *id == c3));
    assert_ne!(mid[0].1, mid[1].1);

    // Whichever of them comes second joins the lane the root commit is
    // already waiting in, via an edge that changes lane (the "merge-in" curve).
    assert!(log[1..3]
        .iter()
        .any(|e| e.graph.outgoing.iter().any(|edge| edge.from != edge.to)));

    // Root commit: a single lane remains.
    let last = log.last().unwrap();
    assert_eq!(last.commit.id, c1);
    assert_eq!(last.graph.converging.len(), 1);
    assert_eq!(last.graph.converging, vec![last.graph.column]);
}

#[test]
fn log_labels_and_paging() {
    let (_d, repo, [_, c2, _, c4]) = diamond();
    let log = repo.log(0, 100).unwrap();
    let head = &log[0].commit.labels;
    assert_eq!(head.len(), 1);
    assert_eq!(head[0].name, "master");
    assert!(head[0].is_head);
    let feat = log.iter().find(|e| e.commit.id == c2).unwrap();
    assert_eq!(feat.commit.labels[0].name, "feature");
    assert!(!feat.commit.labels[0].is_head);

    // Paging returns the same rows (and graph lanes) as the full log.
    let page = repo.log(1, 2).unwrap();
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].commit.id, log[1].commit.id);
    assert_eq!(page[0].graph, log[1].graph);
    assert_ne!(page[0].commit.id, c4);
}

#[test]
fn commit_detail_and_side_by_side() {
    let (_d, repo, [_, _, c3, _]) = diamond();
    let d = repo.commit_detail(c3).unwrap();
    assert_eq!((d.additions, d.deletions), (1, 1));
    assert_eq!(d.files.len(), 1);
    let f = &d.files[0];
    assert_eq!(f.display_path(), "a.txt");
    assert_eq!(f.status, FileStatus::Modified);

    let rows = side_by_side(&f.hunks[0]);
    assert_eq!(rows.len(), 3); // context, changed pair, context
    let Cell { text, kind, changed, lineno } = rows[1].left.clone().unwrap();
    assert_eq!((text.as_str(), kind, lineno), ("two", LineKind::Removed, 2));
    assert_eq!(changed, Some(0..3));
    let right = rows[1].right.clone().unwrap();
    assert_eq!((right.text.as_str(), right.kind, right.lineno), ("TWO", LineKind::Added, 2));
}

#[test]
fn root_commit_is_all_additions() {
    let (_d, repo, [c1, ..]) = diamond();
    let d = repo.commit_detail(c1).unwrap();
    assert_eq!(d.files.len(), 1);
    assert_eq!(d.files[0].status, FileStatus::Added);
    assert_eq!((d.additions, d.deletions), (3, 0));
}

#[test]
fn unequal_runs_leave_blank_cells() {
    let hunk = Hunk {
        header: String::new(),
        old_start: 1,
        new_start: 1,
        lines: vec![
            line(LineKind::Removed, Some(1), None, "a"),
            line(LineKind::Added, None, Some(1), "x"),
            line(LineKind::Added, None, Some(2), "y"),
        ],
    };
    let rows = side_by_side(&hunk);
    assert_eq!(rows.len(), 2);
    assert!(rows[0].left.is_some() && rows[0].right.is_some());
    assert!(rows[1].left.is_none() && rows[1].right.is_some());
}

fn line(kind: LineKind, o: Option<u32>, n: Option<u32>, text: &str) -> diff::DiffLine {
    diff::DiffLine { kind, old_lineno: o, new_lineno: n, text: text.into() }
}

#[test]
fn intra_line_ranges() {
    assert_eq!(changed_ranges("let x = 1;", "let x = 22;"), (8..9, 8..10));
    assert_eq!(changed_ranges("same", "same"), (4..4, 4..4));
    assert_eq!(changed_ranges("", "new"), (0..0, 0..3));
    // Overlapping prefix/suffix must not underflow.
    assert_eq!(changed_ranges("aa", "aaa"), (2..2, 2..3));
}

#[test]
fn refs_listing() {
    let (_d, mut repo, _) = diamond();
    let refs = repo.refs().unwrap();
    assert_eq!(refs.branches, vec!["feature", "master"]);
    assert!(refs.tags.is_empty() && refs.stashes.is_empty());
}

#[test]
fn status_summary_tracks_clean_and_dirty() {
    let (dir, repo) = init();
    std::fs::write(dir.path().join("f.txt"), "hi\n").unwrap();

    let r = Repo::discover(dir.path()).unwrap();
    let s = r.status_summary().unwrap();
    assert_eq!(s.branch.as_deref(), Some("master")); // unborn branch still has a name
    assert!(s.index_clean && !s.working_clean); // untracked file

    let mut index = repo.index().unwrap();
    index.add_path(std::path::Path::new("f.txt")).unwrap();
    index.write().unwrap();
    let s = r.status_summary().unwrap();
    assert!(!s.index_clean && s.working_clean);

    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    repo.commit(Some("HEAD"), &sig(1), &sig(1), "init", &tree, &[]).unwrap();
    let s = r.status_summary().unwrap();
    assert!(s.index_clean && s.working_clean && !s.detached);

    std::fs::write(dir.path().join("f.txt"), "hi\nthere\n").unwrap();
    let entries = r.status().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].worktree, Some(ChangeState::Modified));
    let wd = r.working_diff(false).unwrap();
    assert_eq!((wd[0].additions, wd[0].deletions), (1, 0));
}
