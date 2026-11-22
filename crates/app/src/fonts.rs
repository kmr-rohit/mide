//! Monospace font settings for the editor, terminal and diffs.
//!
//! Defaults to Monaco 12 (Apple's classic coding font, installed on every Mac),
//! falling back to Menlo if it is missing. Configure in `config.toml`:
//!
//! ```toml
//! [editor]
//! font_family = "Monaco"
//! font_size = 12
//!
//! [terminal]            # optional; follows [editor] when omitted
//! font_family = "Menlo"
//! font_size = 13
//! ```

use std::cell::RefCell;

use gpui::SharedString;

pub const DEFAULT_FAMILY: &str = "Monaco";
pub const DEFAULT_SIZE: f32 = 12.0;

/// Tried in order when the requested family isn't installed.
const FALLBACKS: &[&str] = &["Monaco", "Menlo", "SF Mono", "Courier New"];

#[derive(Debug, Clone, PartialEq)]
pub struct Fonts {
    pub editor_family: SharedString,
    pub editor_size: f32,
    pub term_family: SharedString,
    pub term_size: f32,
}

impl Default for Fonts {
    fn default() -> Self {
        Self {
            editor_family: DEFAULT_FAMILY.into(),
            editor_size: DEFAULT_SIZE,
            term_family: DEFAULT_FAMILY.into(),
            term_size: DEFAULT_SIZE,
        }
    }
}

thread_local! {
    static CURRENT: RefCell<Fonts> = RefCell::new(Fonts::default());
}

pub fn get() -> Fonts {
    CURRENT.with(|c| c.borrow().clone())
}

pub fn set(f: Fonts) {
    CURRENT.with(|c| *c.borrow_mut() = f);
}

pub fn editor_family() -> SharedString {
    CURRENT.with(|c| c.borrow().editor_family.clone())
}

pub fn editor_size() -> f32 {
    CURRENT.with(|c| c.borrow().editor_size)
}

pub fn term_family() -> SharedString {
    CURRENT.with(|c| c.borrow().term_family.clone())
}

pub fn term_size() -> f32 {
    CURRENT.with(|c| c.borrow().term_size)
}

/// Line height that keeps the proportions of the reference design (17px at 12pt).
pub fn line_height_for(size: f32) -> f32 {
    (size * 17.0 / 12.0).round().max(10.0)
}

pub fn line_height() -> f32 {
    line_height_for(editor_size())
}

pub fn term_line_height() -> f32 {
    line_height_for(term_size())
}

/// The font to use for `requested`: the family itself if `exists` says it is installed,
/// else the first available fallback, else the request unchanged (the platform then
/// picks something).
pub fn resolve_with(requested: &str, exists: impl Fn(&str) -> bool) -> String {
    std::iter::once(requested)
        .chain(FALLBACKS.iter().copied())
        .find(|name| exists(name))
        .unwrap_or(requested)
        .to_owned()
}

/// [`resolve_with`] against a list of installed family names (matched case-insensitively,
/// returning the installed spelling).
#[cfg(test)]
pub fn resolve(requested: &str, available: &[String]) -> String {
    std::iter::once(requested)
        .chain(FALLBACKS.iter().copied())
        .find_map(|want| available.iter().find(|name| name.eq_ignore_ascii_case(want)).cloned())
        .unwrap_or_else(|| requested.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn default_is_monaco_12() {
        let f = Fonts::default();
        assert_eq!(f.editor_family.as_ref(), "Monaco");
        assert_eq!((f.editor_size, f.term_size), (12.0, 12.0));
    }

    #[test]
    fn resolves_installed_family_case_insensitively() {
        let avail = names(&["Helvetica", "Monaco", "Menlo"]);
        assert_eq!(resolve("Monaco", &avail), "Monaco");
        assert_eq!(resolve("monaco", &avail), "Monaco");
        assert_eq!(resolve("MENLO", &avail), "Menlo");
    }

    #[test]
    fn falls_back_when_missing() {
        // Ubuntu Mono isn't on a Mac by default: use the next best installed font.
        assert_eq!(resolve("Ubuntu Mono", &names(&["Menlo", "Helvetica"])), "Menlo");
        assert_eq!(resolve("Ubuntu Mono", &names(&["Courier New"])), "Courier New");
        // Nothing suitable at all: hand the request back unchanged.
        assert_eq!(resolve("Ubuntu Mono", &names(&["Zapfino"])), "Ubuntu Mono");
        // An installed request beats the fallbacks.
        assert_eq!(resolve("Ubuntu Mono", &names(&["Menlo", "Ubuntu Mono"])), "Ubuntu Mono");
    }

    #[test]
    fn resolve_with_uses_the_probe_in_order() {
        let installed = ["Menlo", "Courier New"];
        let probe = |n: &str| installed.contains(&n);
        assert_eq!(resolve_with("Menlo", probe), "Menlo");
        assert_eq!(resolve_with("Ubuntu Mono", probe), "Menlo", "Monaco is missing here, Menlo is next");
        assert_eq!(resolve_with("Zapfino", |_| false), "Zapfino");
    }

    #[test]
    fn line_height_scales_with_size() {
        assert_eq!(line_height_for(12.0), 17.0);
        assert!(line_height_for(14.0) > line_height_for(12.0));
        assert!(line_height_for(1.0) >= 10.0);
    }

    #[test]
    fn globals_round_trip() {
        set(Fonts { editor_family: "Menlo".into(), editor_size: 14.0, term_family: "Monaco".into(), term_size: 11.0 });
        assert_eq!(editor_family().as_ref(), "Menlo");
        assert_eq!(line_height(), line_height_for(14.0));
        assert_eq!(term_line_height(), line_height_for(11.0));
        set(Fonts::default());
    }
}
