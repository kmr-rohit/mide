//! File-system layer: the sidebar tree model, the flat file list for Cmd+P, and
//! a debounced watcher. No UI code here.

mod watch;

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::WalkBuilder;

pub use watch::{Changes, Watcher};

/// Names never shown anywhere.
fn is_hidden_always(name: &str) -> bool {
    name == ".git" || name == ".DS_Store"
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
    ignored: bool,
}

/// One visible line of the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    /// 0 for the root folder.
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    /// Matched by `.gitignore` (UI draws these dimmed, like Sublime).
    pub ignored: bool,
}

pub struct FileTree {
    root: PathBuf,
    /// Loaded directory listings, sorted.
    children: HashMap<PathBuf, Vec<Entry>>,
    expanded: HashSet<PathBuf>,
    ignore: Gitignore,
}

impl FileTree {
    /// Open `root` with only the root level loaded and expanded.
    pub fn open(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = root.as_ref().canonicalize()?;
        let mut b = GitignoreBuilder::new(&root);
        // Errors here only mean the file is missing or has a bad line.
        let _ = b.add(root.join(".gitignore"));
        let _ = b.add(root.join(".git/info/exclude"));
        let ignore = b.build().unwrap_or_else(|_| Gitignore::empty());

        let mut tree = Self { root: root.clone(), children: HashMap::new(), expanded: HashSet::new(), ignore };
        tree.load(&root)?;
        tree.expanded.insert(root);
        Ok(tree)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn load(&mut self, dir: &Path) -> io::Result<()> {
        let mut entries = Vec::new();
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            if is_hidden_always(&name) {
                continue;
            }
            let path = e.path();
            // Follow symlinks for the dir check; a broken link is a file.
            let is_dir = std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
            let ignored = self.ignore.matched_path_or_any_parents(&path, is_dir).is_ignore();
            entries.push(Entry { name, path, is_dir, ignored });
        }
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.name.cmp(&b.name))
        });
        self.children.insert(dir.to_owned(), entries);
        Ok(())
    }

    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

    /// Expand or collapse a directory, loading it on first expansion.
    pub fn toggle(&mut self, path: &Path) -> io::Result<()> {
        if self.expanded.remove(path) {
            return Ok(());
        }
        if !self.children.contains_key(path) {
            self.load(path)?;
        }
        self.expanded.insert(path.to_owned());
        Ok(())
    }

    /// Expand every ancestor of `path` so it becomes visible ("Reveal in sidebar").
    pub fn reveal(&mut self, path: &Path) -> io::Result<()> {
        let Ok(rel) = path.strip_prefix(&self.root) else { return Ok(()) };
        let mut cur = self.root.clone();
        let comps: Vec<_> = rel.components().collect();
        for c in comps.iter().take(comps.len().saturating_sub(1)) {
            cur.push(c);
            if !self.children.contains_key(&cur) {
                self.load(&cur)?;
            }
            self.expanded.insert(cur.clone());
        }
        Ok(())
    }

    /// Re-read a directory after a file-system change. Directories that were
    /// never loaded are ignored; vanished ones are forgotten.
    pub fn refresh(&mut self, dir: &Path) {
        if !self.children.contains_key(dir) {
            return;
        }
        if dir.is_dir() {
            let _ = self.load(dir);
        } else {
            self.forget(dir);
        }
    }

    pub fn apply(&mut self, changes: &Changes) {
        for d in &changes.dirs {
            self.refresh(d);
        }
    }

    fn forget(&mut self, dir: &Path) {
        self.children.retain(|p, _| !p.starts_with(dir));
        self.expanded.retain(|p| !p.starts_with(dir));
    }

    /// The flattened, visible tree: root first, then depth-first.
    pub fn rows(&self) -> Vec<Row> {
        let mut out = Vec::new();
        let name = self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        out.push(Row {
            path: self.root.clone(),
            name,
            depth: 0,
            is_dir: true,
            expanded: true,
            ignored: false,
        });
        self.push_children(&self.root, 1, &mut out);
        out
    }

    fn push_children(&self, dir: &Path, depth: usize, out: &mut Vec<Row>) {
        let Some(entries) = self.children.get(dir) else { return };
        for e in entries {
            let expanded = e.is_dir && self.expanded.contains(&e.path);
            out.push(Row {
                path: e.path.clone(),
                name: e.name.clone(),
                depth,
                is_dir: e.is_dir,
                expanded,
                ignored: e.ignored,
            });
            if expanded {
                self.push_children(&e.path, depth + 1, out);
            }
        }
    }
}

/// Every file under `root` as a `/`-separated relative path, sorted, honoring
/// `.gitignore` (even outside a git repo). Stops after `max` files.
pub fn list_files(root: impl AsRef<Path>, max: usize) -> Vec<String> {
    let root = root.as_ref();
    let mut out = Vec::new();
    let walk = WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|e| !is_hidden_always(&e.file_name().to_string_lossy()))
        .build();
    for entry in walk.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(root) {
            out.push(rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"));
            if out.len() >= max {
                break;
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests;
