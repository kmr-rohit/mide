//! Mutating git operations (stage, commit, push, pull, branch, ...).
//!
//! These run the real `git` binary rather than libgit2: that way your
//! credential helper, SSH agent, hooks and config behave exactly as they do in
//! a terminal. Every call blocks, so callers run them on a background thread.
//! On success they return git's combined output; on failure, its error text.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub type OpResult = Result<String, String>;

const TIMEOUT: Duration = Duration::from_secs(180);

fn run(root: &Path, args: &[&str], stdin: Option<&str>) -> OpResult {
    let mut child = Command::new("git")
        .current_dir(root)
        .args(args)
        // Never wait for a password/editor that has no terminal to appear on.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .env("GIT_MERGE_AUTOEDIT", "no")
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run git: {e}"))?;

    if let (Some(text), Some(mut si)) = (stdin, child.stdin.take()) {
        let _ = si.write_all(text.as_bytes()); // dropped here: EOF for git
    }
    let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let read_out = thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let read_err = thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });

    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("git did not finish within 3 minutes and was stopped".into());
            }
            Ok(None) => thread::sleep(Duration::from_millis(15)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let (so, se) = (read_out.join().unwrap_or_default(), read_err.join().unwrap_or_default());
    let text = [so.trim(), se.trim()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n");
    if status.success() {
        Ok(text)
    } else if text.is_empty() {
        Err(format!("git {} failed ({status})", args.first().copied().unwrap_or("")))
    } else {
        Err(text)
    }
}

fn with_paths<'a>(head: &[&'a str], paths: &'a [String]) -> Vec<&'a str> {
    head.iter().copied().chain(["--"]).chain(paths.iter().map(String::as_str)).collect()
}

pub fn stage(root: &Path, paths: &[String]) -> OpResult {
    run(root, &with_paths(&["add"], paths), None)
}

pub fn stage_all(root: &Path) -> OpResult {
    run(root, &["add", "-A"], None)
}

pub fn unstage(root: &Path, paths: &[String]) -> OpResult {
    // `reset` (unlike `restore --staged`) also works before the first commit.
    run(root, &with_paths(&["reset", "-q"], paths), None)
}

pub fn unstage_all(root: &Path) -> OpResult {
    run(root, &["reset", "-q"], None)
}

/// Throw away local changes to one file. `untracked` files are deleted.
pub fn discard(root: &Path, path: &str, untracked: bool) -> OpResult {
    let paths = [path.to_owned()];
    if untracked {
        run(root, &with_paths(&["clean", "-f"], &paths), None)
    } else {
        run(root, &with_paths(&["checkout"], &paths), None)
    }
}

pub fn commit(root: &Path, message: &str, amend: bool) -> OpResult {
    if message.trim().is_empty() && !amend {
        return Err("Enter a commit message first".into());
    }
    let mut args = vec!["commit", "-F", "-"];
    if amend {
        args.push("--amend");
        if message.trim().is_empty() {
            args.push("--no-edit");
        }
    }
    if message.trim().is_empty() {
        run(root, &args.into_iter().filter(|a| !matches!(*a, "-F" | "-")).collect::<Vec<_>>(), None)
    } else {
        run(root, &args, Some(message))
    }
}

pub fn current_branch(root: &Path) -> Result<String, String> {
    let b = run(root, &["rev-parse", "--abbrev-ref", "HEAD"], None)?;
    if b == "HEAD" {
        Err("Detached HEAD: switch to a branch first".into())
    } else {
        Ok(b)
    }
}

fn has_upstream(root: &Path) -> bool {
    run(root, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"], None).is_ok()
}

/// Push the current branch, setting its upstream to `origin` on first push.
pub fn push(root: &Path) -> OpResult {
    let branch = current_branch(root)?;
    if has_upstream(root) {
        run(root, &["push"], None)
    } else {
        run(root, &["push", "-u", "origin", &branch], None)
    }
}

pub fn pull(root: &Path) -> OpResult {
    run(root, &["pull", "--no-edit"], None)
}

pub fn fetch(root: &Path) -> OpResult {
    run(root, &["fetch", "--all", "--prune"], None)
}

/// Switch to a branch; a remote-only name like `feature` tracks `origin/feature`.
pub fn switch_branch(root: &Path, name: &str) -> OpResult {
    run(root, &["switch", name], None)
}

pub fn checkout_detached(root: &Path, rev: &str) -> OpResult {
    run(root, &["checkout", "--detach", rev], None)
}

pub fn create_branch(root: &Path, name: &str, from: Option<&str>) -> OpResult {
    let name = name.trim();
    run(root, &["check-ref-format", "--branch", name], None).map_err(|_| format!("'{name}' is not a valid branch name"))?;
    match from {
        Some(rev) => run(root, &["switch", "-c", name, rev], None),
        None => run(root, &["switch", "-c", name], None),
    }
}

pub fn merge(root: &Path, branch: &str) -> OpResult {
    run(root, &["merge", "--no-edit", branch], None)
}

/// Stash tracked and untracked changes.
pub fn stash_push(root: &Path, message: &str) -> OpResult {
    if message.trim().is_empty() {
        run(root, &["stash", "push", "-u"], None)
    } else {
        run(root, &["stash", "push", "-u", "-m", message.trim()], None)
    }
}

pub fn stash_pop(root: &Path) -> OpResult {
    run(root, &["stash", "pop"], None)
}

pub fn cherry_pick(root: &Path, rev: &str) -> OpResult {
    run(root, &["cherry-pick", rev], None)
}

pub fn revert(root: &Path, rev: &str) -> OpResult {
    run(root, &["revert", "--no-edit", rev], None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn git_ok(dir: &Path, args: &[&str]) -> String {
        run(dir, args, None).unwrap_or_else(|e| panic!("git {args:?}: {e}"))
    }

    fn repo() -> TempDir {
        let d = tempfile::tempdir().unwrap();
        git_ok(d.path(), &["init", "-q", "-b", "master"]);
        for (k, v) in [("user.name", "T"), ("user.email", "t@example.com"), ("commit.gpgsign", "false")] {
            git_ok(d.path(), &["config", k, v]);
        }
        d
    }

    fn write(d: &Path, name: &str, text: &str) {
        fs::write(d.join(name), text).unwrap();
    }

    fn files(s: &str) -> Vec<String> {
        s.lines().map(str::to_owned).collect()
    }

    fn subjects(d: &Path) -> Vec<String> {
        files(&git_ok(d, &["log", "--format=%s"]))
    }

    #[test]
    fn stage_commit_amend_and_unstage() {
        let d = repo();
        let p = d.path();
        write(p, "a.txt", "1\n");
        write(p, "b.txt", "1\n");
        stage(p, &["a.txt".into()]).unwrap();
        assert_eq!(git_ok(p, &["diff", "--cached", "--name-only"]), "a.txt");

        unstage(p, &["a.txt".into()]).unwrap(); // works before the first commit
        assert_eq!(git_ok(p, &["diff", "--cached", "--name-only"]), "");

        stage_all(p).unwrap();
        commit(p, "first\n\nmulti-line body", false).unwrap();
        assert_eq!(git_ok(p, &["log", "-1", "--format=%B"]).trim(), "first\n\nmulti-line body");

        write(p, "a.txt", "2\n");
        stage_all(p).unwrap();
        commit(p, "", true).unwrap(); // amend, keep message
        assert_eq!(subjects(p), ["first"]);
        write(p, "a.txt", "3\n");
        stage_all(p).unwrap();
        commit(p, "renamed", true).unwrap();
        assert_eq!(subjects(p), ["renamed"]);

        write(p, "a.txt", "4\n");
        stage_all(p).unwrap();
        unstage_all(p).unwrap();
        assert_eq!(git_ok(p, &["diff", "--cached", "--name-only"]), "");
    }

    #[test]
    fn commit_errors_are_readable() {
        let d = repo();
        assert!(commit(d.path(), "   ", false).unwrap_err().contains("message"));
        // Nothing staged.
        assert!(commit(d.path(), "msg", false).is_err());
    }

    #[test]
    fn discard_tracked_and_untracked() {
        let d = repo();
        let p = d.path();
        write(p, "a.txt", "orig\n");
        stage_all(p).unwrap();
        commit(p, "init", false).unwrap();
        write(p, "a.txt", "changed\n");
        write(p, "new.txt", "x\n");
        discard(p, "a.txt", false).unwrap();
        discard(p, "new.txt", true).unwrap();
        assert_eq!(fs::read_to_string(p.join("a.txt")).unwrap(), "orig\n");
        assert!(!p.join("new.txt").exists());
    }

    #[test]
    fn branches_merge_stash_pick_revert() {
        let d = repo();
        let p = d.path();
        write(p, "a.txt", "base\n");
        stage_all(p).unwrap();
        commit(p, "base", false).unwrap();

        assert!(create_branch(p, "bad name", None).unwrap_err().contains("not a valid branch name"));
        create_branch(p, "feature", None).unwrap();
        assert_eq!(current_branch(p).unwrap(), "feature");
        write(p, "f.txt", "f\n");
        stage_all(p).unwrap();
        commit(p, "feature work", false).unwrap();
        let feature_tip = git_ok(p, &["rev-parse", "HEAD"]);

        switch_branch(p, "master").unwrap();
        assert!(!p.join("f.txt").exists());
        merge(p, "feature").unwrap();
        assert!(p.join("f.txt").exists());

        // Revert undoes it; cherry-pick on another branch brings it back.
        revert(p, &feature_tip).unwrap();
        assert!(!p.join("f.txt").exists());
        create_branch(p, "other", Some("HEAD~2")).unwrap();
        cherry_pick(p, &feature_tip).unwrap();
        assert!(p.join("f.txt").exists());

        // Stash includes untracked files, and pop restores them.
        write(p, "scratch.txt", "s\n");
        write(p, "a.txt", "dirty\n");
        stash_push(p, "wip").unwrap();
        assert!(!p.join("scratch.txt").exists());
        assert_eq!(fs::read_to_string(p.join("a.txt")).unwrap(), "base\n");
        stash_pop(p).unwrap();
        assert!(p.join("scratch.txt").exists());

        checkout_detached(p, "master").unwrap();
        assert!(current_branch(p).unwrap_err().contains("Detached"));
    }

    #[test]
    fn push_pull_fetch_against_a_remote() {
        let remote = tempfile::tempdir().unwrap();
        git_ok(remote.path(), &["init", "-q", "--bare", "-b", "master"]);
        let url = remote.path().to_str().unwrap().to_owned();

        let a = repo();
        let pa = a.path();
        git_ok(pa, &["remote", "add", "origin", &url]);
        write(pa, "a.txt", "1\n");
        stage_all(pa).unwrap();
        commit(pa, "one", false).unwrap();
        push(pa).unwrap(); // first push sets the upstream
        assert!(has_upstream(pa));

        // A second clone pushes a commit; the first pulls it.
        let b = tempfile::tempdir().unwrap();
        git_ok(b.path(), &["clone", "-q", &url, "."]);
        for (k, v) in [("user.name", "B"), ("user.email", "b@example.com"), ("commit.gpgsign", "false")] {
            git_ok(b.path(), &["config", k, v]);
        }
        write(b.path(), "b.txt", "2\n");
        stage_all(b.path()).unwrap();
        commit(b.path(), "two", false).unwrap();
        push(b.path()).unwrap();

        fetch(pa).unwrap();
        assert_eq!(git_ok(pa, &["rev-list", "--count", "HEAD..@{u}"]), "1");
        pull(pa).unwrap();
        assert_eq!(subjects(pa), ["two", "one"]);

        // Pushing again with a second local commit works through the existing upstream.
        write(pa, "c.txt", "3\n");
        stage_all(pa).unwrap();
        commit(pa, "three", false).unwrap();
        push(pa).unwrap();
        // `--git-dir` keeps this working under `safe.bareRepository=explicit`.
        assert_eq!(git_ok(remote.path(), &["--git-dir", &url, "log", "-1", "--format=%s"]), "three");
    }

    #[test]
    fn push_without_a_remote_fails_with_git_message() {
        let d = repo();
        write(d.path(), "a.txt", "1\n");
        stage_all(d.path()).unwrap();
        commit(d.path(), "one", false).unwrap();
        let e = push(d.path()).unwrap_err();
        assert!(e.contains("origin"), "got: {e}");
    }
}
