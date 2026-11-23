//! Remembering what was open, per project, and keeping unsaved text safe.
//!
//! * **Session**: which tabs were open, which one was active, and where each caret and
//!   scroll position was. Stored at `~/.config/mide/sessions/<project>.toml` and
//!   restored when the project is opened again.
//! * **Backups**: unsaved text is copied to `~/.config/mide/backups/<project>/` every few
//!   seconds, and removed on save or close. After a crash or a force quit the next launch
//!   offers that text back instead of the (older) file on disk.
//!
//! Everything here is best effort: failing to write must never get in the user's way.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TabState {
    pub path: PathBuf,
    /// 0-based caret position.
    pub line: usize,
    pub col: usize,
    pub scroll_y: f32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionState {
    #[serde(default)]
    pub tabs: Vec<TabState>,
    #[serde(default)]
    pub active: usize,
}

/// A stable short name for a path (FNV-1a), used as a file name.
pub fn key(path: &Path) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in path.to_string_lossy().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn base() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".config/mide"))
}

/// Where state lives. Tests point this at a temp directory.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn default_store() -> Option<Self> {
        base().map(|dir| Self { dir })
    }

    #[cfg(test)]
    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn session_file(&self, root: &Path) -> PathBuf {
        self.dir.join("sessions").join(format!("{}.toml", key(root)))
    }

    fn backup_dir(&self, root: &Path) -> PathBuf {
        self.dir.join("backups").join(key(root))
    }

    pub fn save_session(&self, root: &Path, state: &SessionState) {
        let file = self.session_file(root);
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = toml::to_string(state) {
            let _ = std::fs::write(file, text);
        }
    }

    /// Tabs whose files no longer exist are dropped.
    pub fn load_session(&self, root: &Path) -> SessionState {
        let text = std::fs::read_to_string(self.session_file(root)).unwrap_or_default();
        let mut s: SessionState = toml::from_str(&text).unwrap_or_default();
        let active_path = s.tabs.get(s.active).map(|t| t.path.clone());
        s.tabs.retain(|t| t.path.is_file());
        s.active = active_path.and_then(|p| s.tabs.iter().position(|t| t.path == p)).unwrap_or(0);
        s
    }

    // ---- unsaved-text backups -------------------------------------------------------

    /// Backups are stored as `<key>.txt` (the text) next to `<key>.path` (which file it is for).
    pub fn write_backup(&self, root: &Path, file: &Path, text: &str) {
        let dir = self.backup_dir(root);
        let _ = std::fs::create_dir_all(&dir);
        let k = key(file);
        let _ = std::fs::write(dir.join(format!("{k}.path")), file.to_string_lossy().as_bytes());
        let _ = std::fs::write(dir.join(format!("{k}.txt")), text);
    }

    pub fn clear_backup(&self, root: &Path, file: &Path) {
        let dir = self.backup_dir(root);
        let k = key(file);
        let _ = std::fs::remove_file(dir.join(format!("{k}.txt")));
        let _ = std::fs::remove_file(dir.join(format!("{k}.path")));
    }

    /// Every backed-up file of this project with its saved text.
    pub fn backups(&self, root: &Path) -> Vec<(PathBuf, String)> {
        let Ok(rd) = std::fs::read_dir(self.backup_dir(root)) else { return Vec::new() };
        let mut out = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "path") {
                continue;
            }
            let Ok(file) = std::fs::read_to_string(&p) else { continue };
            if let Ok(text) = std::fs::read_to_string(p.with_extension("txt")) {
                out.push((PathBuf::from(file), text));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mide-session-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn key_is_stable_and_distinct() {
        assert_eq!(key(Path::new("/a/b")), key(Path::new("/a/b")));
        assert_ne!(key(Path::new("/a/b")), key(Path::new("/a/c")));
    }

    #[test]
    fn session_round_trips_and_drops_missing_files() {
        let dir = temp("rt");
        let store = Store::at(dir.join("cfg"));
        let (a, b, gone) = (dir.join("a.rs"), dir.join("b.rs"), dir.join("gone.rs"));
        std::fs::write(&a, "x").unwrap();
        std::fs::write(&b, "y").unwrap();
        let root = dir.join("proj");
        let state = SessionState {
            tabs: vec![
                TabState { path: a.clone(), line: 3, col: 4, scroll_y: 10.0 },
                TabState { path: gone, line: 0, col: 0, scroll_y: 0.0 },
                TabState { path: b.clone(), line: 9, col: 1, scroll_y: 0.0 },
            ],
            active: 2,
        };
        store.save_session(&root, &state);
        let back = store.load_session(&root);
        assert_eq!(back.tabs.len(), 2);
        assert_eq!(back.tabs[0].path, a);
        assert_eq!(back.tabs[0].line, 3);
        // The active tab is followed to its new index after a dropped tab.
        assert_eq!(back.active, 1);
        assert_eq!(back.tabs[back.active].path, b);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_or_corrupt_session_is_empty() {
        let dir = temp("bad");
        let store = Store::at(dir.join("cfg"));
        assert_eq!(store.load_session(Path::new("/nope")), SessionState::default());
        let root = Path::new("/some/root");
        let f = store.session_file(root);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, "tabs = [[[ not toml").unwrap();
        assert_eq!(store.load_session(root), SessionState::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn backups_round_trip_and_clear() {
        let dir = temp("bk");
        let store = Store::at(dir.join("cfg"));
        let root = dir.join("proj");
        let f = dir.join("notes.txt");
        store.write_backup(&root, &f, "unsaved\nwork");
        store.write_backup(&root, &f, "unsaved\nwork v2");
        assert_eq!(store.backups(&root), vec![(f.clone(), "unsaved\nwork v2".to_owned())]);
        store.clear_backup(&root, &f);
        assert!(store.backups(&root).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
