//! Embedded SVG icons, served to GPUI's `svg()` element.

use std::borrow::Cow;

use anyhow::Result;
use gpui::{AssetSource, SharedString};

pub struct Assets;

const ICONS: &[(&str, &str)] = &[
    ("icons/chevron-right.svg", include_str!("../assets/icons/chevron-right.svg")),
    ("icons/chevron-down.svg", include_str!("../assets/icons/chevron-down.svg")),
    ("icons/folder.svg", include_str!("../assets/icons/folder.svg")),
    ("icons/folder-open.svg", include_str!("../assets/icons/folder-open.svg")),
    ("icons/file.svg", include_str!("../assets/icons/file.svg")),
    ("icons/file-code.svg", include_str!("../assets/icons/file-code.svg")),
    ("icons/file-lines.svg", include_str!("../assets/icons/file-lines.svg")),
];

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS.iter().find(|(p, _)| *p == path).map(|(_, svg)| Cow::Borrowed(svg.as_bytes())))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| SharedString::from(*p))
            .collect())
    }
}

/// Which icon a file gets in the sidebar (matches Sublime: code files get the
/// `/*` glyph, text-like files get lines, everything else a plain page).
pub fn file_icon(name: &str) -> &'static str {
    const CODE: &[&str] = &[
        "rs", "py", "js", "mjs", "ts", "tsx", "jsx", "go", "c", "h", "cc", "cpp", "hpp", "java", "kt",
        "swift", "rb", "sh", "zsh", "bash", "yaml", "yml", "json", "toml", "html", "css", "scss", "xml",
        "sql", "lua", "vim", "php", "cs", "scala", "ex", "exs", "pl",
    ];
    const CODE_NAMES: &[&str] = &["Jenkinsfile", "Makefile", "CMakeLists.txt", "Rakefile", "Gemfile"];
    const LINES: &[&str] = &["md", "txt", "lock", "log", "ini", "cfg", "conf", "csv", "rst"];
    const LINE_NAMES: &[&str] = &[".gitignore", ".gitattributes", "LICENSE", "README", ".dockerignore", ".editorconfig"];

    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    if CODE_NAMES.contains(&name) || ext.as_deref().is_some_and(|e| CODE.contains(&e)) {
        "icons/file-code.svg"
    } else if LINE_NAMES.contains(&name) || ext.as_deref().is_some_and(|e| LINES.contains(&e)) {
        "icons/file-lines.svg"
    } else {
        "icons/file.svg"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_embedded_and_valid_svg() {
        for (path, _) in ICONS {
            let bytes = Assets.load(path).unwrap().unwrap();
            let text = std::str::from_utf8(&bytes).unwrap();
            assert!(text.contains("<svg") && text.contains("</svg>"), "{path}");
        }
        assert!(Assets.load("icons/nope.svg").unwrap().is_none());
    }

    #[test]
    fn file_icon_choice_matches_sublime() {
        assert_eq!(file_icon("deploy.yaml"), "icons/file-code.svg");
        assert_eq!(file_icon("Jenkinsfile"), "icons/file-code.svg");
        assert_eq!(file_icon(".gitignore"), "icons/file-lines.svg");
        assert_eq!(file_icon("README.md"), "icons/file-lines.svg");
        assert_eq!(file_icon("Dockerfile"), "icons/file.svg");
        assert_eq!(file_icon("image.png"), "icons/file.svg");
    }
}
