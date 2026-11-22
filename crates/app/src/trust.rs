//! Workspace trust. A project's `.mide/config.toml` can define terminal profiles and
//! sessions to auto-start, which means *running commands*. Opening a repository you
//! cloned from somewhere must never do that by itself, so those parts of a project
//! config are ignored until you explicitly trust the folder.
//!
//! Trusted folders are stored one per line in `~/.config/mide/trusted.txt`.
//! Trusting a folder also trusts everything inside it.

use std::path::{Path, PathBuf};

fn store_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/mide/trusted.txt"))
}

pub fn parse(text: &str) -> Vec<PathBuf> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(PathBuf::from).collect()
}

/// Is `root` one of the trusted folders, or inside one?
pub fn contains(trusted: &[PathBuf], root: &Path) -> bool {
    trusted.iter().any(|t| root.starts_with(t))
}

pub fn load() -> Vec<PathBuf> {
    store_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| parse(&t)).unwrap_or_default()
}

pub fn is_trusted(root: &Path) -> bool {
    contains(&load(), root)
}

/// Remember `root` as trusted. Returns an error message if it could not be saved.
pub fn trust(root: &Path) -> Result<(), String> {
    let path = store_path().ok_or("no home directory")?;
    let mut list = load();
    if !contains(&list, root) {
        list.push(root.to_owned());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text: String = list.iter().map(|p| format!("{}\n", p.display())).collect();
    std::fs::write(path, text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusting_a_folder_trusts_what_is_inside_it() {
        let list = vec![PathBuf::from("/work/projects"), PathBuf::from("/tmp/one")];
        assert!(contains(&list, Path::new("/work/projects")));
        assert!(contains(&list, Path::new("/work/projects/app/sub")));
        assert!(contains(&list, Path::new("/tmp/one")));
        assert!(!contains(&list, Path::new("/work/projects-evil")), "a shared name prefix is not a parent");
        assert!(!contains(&list, Path::new("/work")));
        assert!(!contains(&[], Path::new("/anything")));
    }

    #[test]
    fn parsing_skips_blanks_and_comments() {
        assert_eq!(parse("# my projects\n/a\n\n  /b  \n"), [PathBuf::from("/a"), PathBuf::from("/b")]);
    }
}
