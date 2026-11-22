use super::*;
use std::fs;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn touch(root: &Path, rel: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, "x").unwrap();
}

fn project() -> TempDir {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    for f in [
        "Jenkinsfile", "Dockerfile", ".gitignore", "README.md", ".DS_Store",
        ".config/a.yaml", "config/b.yaml", "manifest/one.yaml", "manifest/two.yaml",
        "src/demo/main.py", "src/demo/util.py", "build/out.bin", "Zeta/z.txt", "alpha/a.txt",
        ".git/HEAD",
    ] {
        touch(r, f);
    }
    fs::write(r.join(".gitignore"), "build/\n*.log\n").unwrap();
    d
}

fn names(rows: &[Row]) -> Vec<String> {
    rows.iter().map(|r| format!("{}{}{}", "  ".repeat(r.depth), r.name, if r.is_dir { "/" } else { "" })).collect()
}

#[test]
fn root_level_sorted_dirs_first_case_insensitive_and_hides_git() {
    let d = project();
    let t = FileTree::open(d.path()).unwrap();
    let rows = t.rows();
    let n = names(&rows);
    assert!(n[0].ends_with('/') && rows[0].depth == 0 && rows[0].expanded);
    assert_eq!(
        &n[1..],
        [
            "  .config/", "  alpha/", "  build/", "  config/", "  manifest/", "  src/", "  Zeta/",
            "  .gitignore", "  Dockerfile", "  Jenkinsfile", "  README.md"
        ]
    );
}

#[test]
fn lazy_expand_collapse_and_reveal() {
    let d = project();
    let mut t = FileTree::open(d.path()).unwrap();
    let manifest = t.root().join("manifest");
    assert!(!names(&t.rows()).iter().any(|n| n.contains("one.yaml")));

    t.toggle(&manifest).unwrap();
    let rows = t.rows();
    let i = rows.iter().position(|r| r.name == "manifest").unwrap();
    assert!(rows[i].expanded);
    assert_eq!((rows[i + 1].name.as_str(), rows[i + 1].depth), ("one.yaml", 2));

    t.toggle(&manifest).unwrap();
    assert!(!t.rows().iter().any(|r| r.name == "one.yaml"));

    // Reveal expands every ancestor.
    t.reveal(&t.root().join("src/demo/util.py")).unwrap();
    let rows = t.rows();
    assert!(rows.iter().any(|r| r.name == "util.py" && r.depth == 3));
}

#[test]
fn gitignored_entries_are_flagged_not_hidden() {
    let d = project();
    let t = FileTree::open(d.path()).unwrap();
    let rows = t.rows();
    assert!(rows.iter().find(|r| r.name == "build").unwrap().ignored);
    assert!(!rows.iter().find(|r| r.name == "src").unwrap().ignored);
}

#[test]
fn refresh_picks_up_new_and_removed_entries() {
    let d = project();
    let mut t = FileTree::open(d.path()).unwrap();
    let manifest = t.root().join("manifest");
    t.toggle(&manifest).unwrap();

    touch(t.root(), "manifest/three.yaml");
    fs::remove_file(t.root().join("manifest/one.yaml")).unwrap();
    t.refresh(&manifest);
    let n: Vec<_> = t.rows().into_iter().filter(|r| r.depth == 2).map(|r| r.name).collect();
    assert_eq!(n, ["three.yaml", "two.yaml"]);

    // Removing an expanded directory forgets it entirely.
    fs::remove_dir_all(&manifest).unwrap();
    t.refresh(&manifest);
    t.refresh(t.root().to_owned().as_path());
    assert!(!t.rows().iter().any(|r| r.name == "manifest"));
    assert!(!t.is_expanded(&manifest));
}

#[test]
fn list_files_honors_gitignore_and_skips_git_dir() {
    let d = project();
    fs::write(d.path().join("debug.log"), "x").unwrap();
    let files = list_files(d.path(), 1000);
    assert!(files.contains(&"src/demo/main.py".to_owned()));
    assert!(files.contains(&".config/a.yaml".to_owned())); // dotfiles are included
    assert!(!files.iter().any(|f| f.starts_with("build/") || f.ends_with(".log")));
    assert!(!files.iter().any(|f| f.starts_with(".git/") || f.contains(".DS_Store")));
    assert!(files.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(list_files(d.path(), 3).len(), 3);
}

#[test]
fn watcher_reports_changes_and_git_activity() {
    let d = project();
    let root = d.path().canonicalize().unwrap();
    let (_w, rx) = Watcher::new(&root).unwrap();
    std::thread::sleep(Duration::from_millis(300)); // let FSEvents settle

    touch(&root, "manifest/new.yaml");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = Changes::default();
    while Instant::now() < deadline && !seen.paths.contains(&root.join("manifest/new.yaml")) {
        if let Ok(c) = rx.recv_timeout(Duration::from_millis(200)) {
            seen.paths.extend(c.paths);
            seen.dirs.extend(c.dirs);
        }
    }
    assert!(seen.paths.contains(&root.join("manifest/new.yaml")), "got {seen:?}");
    assert!(seen.dirs.contains(&root.join("manifest")));

    // Feeding the batch to a tree refreshes it.
    let mut t = FileTree::open(&root).unwrap();
    t.toggle(&root.join("manifest")).unwrap();
    touch(&root, "manifest/newer.yaml");
    t.apply(&Changes { dirs: vec![root.join("manifest")], ..Default::default() });
    assert!(t.rows().iter().any(|r| r.name == "newer.yaml"));

    // Ref updates set `git`; object writes alone don't produce a batch.
    fs::write(root.join(".git/HEAD"), "ref: refs/heads/other\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut git = false;
    while Instant::now() < deadline && !git {
        if let Ok(c) = rx.recv_timeout(Duration::from_millis(200)) {
            git |= c.git;
        }
    }
    assert!(git);
}

#[test]
fn summarize_filters_noise() {
    let git = PathBuf::from("/p/.git");
    let raw = [
        PathBuf::from("/p/.git/objects/ab/cdef"),
        PathBuf::from("/p/.git/index.lock"),
        PathBuf::from("/p/.DS_Store"),
        PathBuf::from("/p/src/a.rs"),
    ];
    let s = watch::summarize_for_test(&raw, &git);
    assert!(!s.git);
    assert_eq!(s.paths, [PathBuf::from("/p/src/a.rs")]);
    let s = watch::summarize_for_test(&[PathBuf::from("/p/.git/HEAD")], &git);
    assert!(s.git && s.paths.is_empty());
}
