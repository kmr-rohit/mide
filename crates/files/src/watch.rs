use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use notify::{RecursiveMode, Watcher as _};

/// A debounced batch of file-system changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// Directories whose contents changed (parents of the touched paths).
    pub dirs: Vec<PathBuf>,
    /// Every touched path outside `.git`, for reloading open files.
    pub paths: Vec<PathBuf>,
    /// HEAD, the index or a ref moved: git status / log should refresh.
    pub git: bool,
}

/// Recursive watcher on a project root. Dropping it stops watching.
pub struct Watcher {
    _inner: notify::RecommendedWatcher,
}

const QUIET: Duration = Duration::from_millis(80);

impl Watcher {
    /// Start watching `root`. Batches arrive on the returned receiver once the
    /// file system has been quiet for ~80ms.
    pub fn new(root: impl AsRef<Path>) -> notify::Result<(Self, Receiver<Changes>)> {
        let root = root.as_ref().canonicalize().map_err(notify::Error::io)?;
        let (raw_tx, raw_rx) = mpsc::channel::<PathBuf>();
        let mut inner = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                for p in ev.paths {
                    let _ = raw_tx.send(p);
                }
            }
        })?;
        inner.watch(&root, RecursiveMode::Recursive)?;

        let (tx, rx) = mpsc::channel();
        let git_dir = root.join(".git");
        std::thread::spawn(move || {
            // Ends when the watcher (and its sender) is dropped.
            while let Ok(first) = raw_rx.recv() {
                let mut batch = vec![first];
                while let Ok(p) = raw_rx.recv_timeout(QUIET) {
                    batch.push(p);
                }
                let changes = summarize(&batch, &git_dir);
                if (!changes.paths.is_empty() || changes.git) && tx.send(changes).is_err() {
                    break;
                }
            }
        });
        Ok((Self { _inner: inner }, rx))
    }
}

fn summarize(raw: &[PathBuf], git_dir: &Path) -> Changes {
    let mut dirs = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut git = false;
    for p in raw {
        if let Ok(rel) = p.strip_prefix(git_dir) {
            // Object writes are noise; HEAD/index/refs are what matter.
            git |= !rel.starts_with("objects") && rel.extension().is_none_or(|e| e != "lock");
            continue;
        }
        if p.file_name().is_some_and(|n| n == ".DS_Store") {
            continue;
        }
        if let Some(parent) = p.parent() {
            dirs.insert(parent.to_owned());
        }
        // A directory created/removed also changes its own listing.
        dirs.insert(p.clone());
        paths.insert(p.clone());
    }
    Changes { dirs: dirs.into_iter().collect(), paths: paths.into_iter().collect(), git }
}

#[cfg(test)]
pub(crate) fn summarize_for_test(raw: &[PathBuf], git_dir: &Path) -> Changes {
    summarize(raw, git_dir)
}
