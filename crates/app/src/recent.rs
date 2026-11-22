//! Recently opened folders, stored one per line in `~/.config/mide/recent.txt`.

use std::path::{Path, PathBuf};

pub const MAX_RECENT: usize = 8;

fn store_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/mide/recent.txt"))
}

/// Put `path` first, dropping any earlier occurrence, capped at `max`.
pub fn push(list: &mut Vec<PathBuf>, path: &Path, max: usize) {
    list.retain(|p| p != path);
    list.insert(0, path.to_owned());
    list.truncate(max);
}

pub fn parse(text: &str) -> Vec<PathBuf> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).map(PathBuf::from).collect()
}

fn render(list: &[PathBuf]) -> String {
    list.iter().map(|p| format!("{}\n", p.display())).collect()
}

/// Load the list, skipping folders that no longer exist.
pub fn load() -> Vec<PathBuf> {
    let Some(file) = store_path() else { return Vec::new() };
    let text = std::fs::read_to_string(file).unwrap_or_default();
    parse(&text).into_iter().filter(|p| p.is_dir()).take(MAX_RECENT).collect()
}

/// Best effort: losing the recents list must never get in the way of opening a folder.
pub fn save(list: &[PathBuf]) {
    let Some(file) = store_path() else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, render(list));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_moves_to_front_dedupes_and_caps() {
        let mut l: Vec<PathBuf> = ["/a", "/b", "/c"].iter().map(PathBuf::from).collect();
        push(&mut l, Path::new("/c"), 3);
        assert_eq!(l, [PathBuf::from("/c"), "/a".into(), "/b".into()]);
        push(&mut l, Path::new("/d"), 3);
        assert_eq!(l, [PathBuf::from("/d"), "/c".into(), "/a".into()]);
    }

    #[test]
    fn parse_and_render_round_trip_ignoring_blank_lines() {
        let l = parse("/a\n\n  /b  \n");
        assert_eq!(l, [PathBuf::from("/a"), "/b".into()]);
        assert_eq!(parse(&render(&l)), l);
    }
}
