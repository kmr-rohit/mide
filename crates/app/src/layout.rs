//! Panel layout: sizes and dock position, remembered in `~/.config/mide/layout.toml`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::theme::Mode;

/// Where the terminal panel sits relative to the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dock {
    #[default]
    Bottom,
    Left,
    Right,
}

impl Dock {
    pub fn label(self) -> &'static str {
        match self {
            Dock::Bottom => "bottom",
            Dock::Left => "left",
            Dock::Right => "right",
        }
    }

    /// Bottom -> Right -> Left -> Bottom, for the header's dock button.
    pub fn next(self) -> Self {
        match self {
            Dock::Bottom => Dock::Right,
            Dock::Right => Dock::Left,
            Dock::Left => Dock::Bottom,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    /// Light or dark chrome (the editor's own theme never changes).
    pub theme: Mode,
    pub sidebar_width: f32,
    pub show_sidebar: bool,
    pub terminal_dock: Dock,
    /// Used when docked at the bottom.
    pub terminal_height: f32,
    /// Used when docked left or right.
    pub terminal_width: f32,
    pub git_locations_width: f32,
    pub git_commits_width: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            theme: Mode::Light,
            sidebar_width: 224.0,
            show_sidebar: true,
            terminal_dock: Dock::Bottom,
            terminal_height: 300.0,
            terminal_width: 480.0,
            git_locations_width: 200.0,
            git_commits_width: 380.0,
        }
    }
}

pub const SIDEBAR_RANGE: (f32, f32) = (140.0, 600.0);
pub const TERMINAL_HEIGHT_RANGE: (f32, f32) = (90.0, 1200.0);
pub const TERMINAL_WIDTH_RANGE: (f32, f32) = (200.0, 1400.0);
pub const GIT_PANE_RANGE: (f32, f32) = (120.0, 700.0);

fn clamp(v: f32, (lo, hi): (f32, f32), fallback: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        fallback
    }
}

impl Layout {
    /// Keep hand-edited or corrupt values usable.
    pub fn sanitized(mut self) -> Self {
        let d = Layout::default();
        self.sidebar_width = clamp(self.sidebar_width, SIDEBAR_RANGE, d.sidebar_width);
        self.terminal_height = clamp(self.terminal_height, TERMINAL_HEIGHT_RANGE, d.terminal_height);
        self.terminal_width = clamp(self.terminal_width, TERMINAL_WIDTH_RANGE, d.terminal_width);
        self.git_locations_width = clamp(self.git_locations_width, GIT_PANE_RANGE, d.git_locations_width);
        self.git_commits_width = clamp(self.git_commits_width, GIT_PANE_RANGE, d.git_commits_width);
        self
    }

    fn path() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/mide/layout.toml"))
    }

    pub fn parse(text: &str) -> Self {
        toml::from_str::<Layout>(text).unwrap_or_default().sanitized()
    }

    pub fn load() -> Self {
        Self::path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    /// Best effort: never fail an interaction because the layout couldn't be saved.
    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = toml::to_string_pretty(self) {
            let _ = std::fs::write(path, text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml() {
        let l = Layout { terminal_dock: Dock::Right, sidebar_width: 300.0, show_sidebar: false, theme: Mode::Dark, ..Layout::default() };
        let text = toml::to_string_pretty(&l).unwrap();
        assert!(text.contains("theme = \"dark\""));
        assert!(text.contains("terminal_dock = \"right\""));
        assert_eq!(Layout::parse(&text), l);
    }

    #[test]
    fn partial_and_garbage_files_fall_back_to_defaults() {
        let l = Layout::parse("sidebar_width = 300.0\n");
        assert_eq!(l.sidebar_width, 300.0);
        assert_eq!(l.terminal_dock, Dock::Bottom);
        assert_eq!(Layout::parse("this is [not toml"), Layout::default());
        assert_eq!(Layout::parse("terminal_dock = \"sideways\""), Layout::default());
        assert_eq!(Layout::parse("theme = \"dark\"\n").theme, Mode::Dark);
        assert_eq!(Layout::parse("theme = \"neon\"\n"), Layout::default());
    }

    #[test]
    fn absurd_sizes_are_clamped() {
        let l = Layout::parse("sidebar_width = 5.0\nterminal_height = 99999.0\nterminal_width = nan\n");
        assert_eq!(l.sidebar_width, SIDEBAR_RANGE.0);
        assert_eq!(l.terminal_height, TERMINAL_HEIGHT_RANGE.1);
        assert_eq!(l.terminal_width, Layout::default().terminal_width);
    }

    #[test]
    fn dock_cycle_visits_all_three() {
        assert_eq!(Dock::Bottom.next().next().next(), Dock::Bottom);
        assert_ne!(Dock::Bottom.next(), Dock::Bottom.next().next());
    }
}
