//! User key bindings from `~/.config/mide/keymap.toml`, applied on top of the defaults.
//!
//! ```toml
//! [[bind]]
//! key = "cmd-k cmd-d"            # space-separated keystrokes make a chord
//! action = "editor::DuplicateLine"
//! context = "Editor"             # optional: where the binding applies
//!
//! [[bind]]
//! key = "cmd-e"
//! action = "none"                # frees a key
//! ```
//!
//! A later binding for the same key wins over earlier ones, and the file wins over the defaults.

use std::path::PathBuf;
use std::sync::Mutex;

use gpui::{App, KeyBinding, KeyBindingContextPredicate, Keystroke};
use serde::Deserialize;

/// Problems found while loading the file, shown once by the window.
static ERRORS: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn take_errors() -> Vec<String> {
    ERRORS.lock().map(|mut e| std::mem::take(&mut *e)).unwrap_or_default()
}

pub fn path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/mide/keymap.toml"))
}

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct File {
    bind: Vec<Bind>,
}

#[derive(Debug, Default, Deserialize, PartialEq, Clone)]
pub struct Bind {
    pub key: String,
    pub action: String,
    pub context: Option<String>,
}

/// Read the file's text. `Err` is a readable message for malformed TOML.
pub fn parse(text: &str) -> Result<Vec<Bind>, String> {
    toml::from_str::<File>(text).map(|f| f.bind).map_err(|e| e.message().to_owned())
}

/// Namespaces actions live in, so `DuplicateLine` works as well as `editor::DuplicateLine`.
const NAMESPACES: [&str; 4] = ["editor", "workspace", "palette", "git"];

fn build(cx: &App, b: &Bind) -> Result<KeyBinding, String> {
    let key = b.key.trim();
    if key.is_empty() {
        return Err("a binding has no key".into());
    }
    for part in key.split_whitespace() {
        Keystroke::parse(part).map_err(|_| format!("“{key}” is not a valid key"))?;
    }
    let name = b.action.trim();
    let action = if name.eq_ignore_ascii_case("none") {
        cx.build_action("zed::NoAction", None).map_err(|_| "cannot unbind keys here".to_owned())?
    } else if name.contains("::") {
        cx.build_action(name, None).map_err(|_| format!("unknown action “{name}”"))?
    } else {
        NAMESPACES
            .iter()
            .find_map(|ns| cx.build_action(&format!("{ns}::{name}"), None).ok())
            .ok_or_else(|| format!("unknown action “{name}”"))?
    };
    let context = match b.context.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => Some(KeyBindingContextPredicate::parse(c).map_err(|e| format!("bad context “{c}”: {e}"))?.into()),
        None => None,
    };
    KeyBinding::load(key, action, context, false, None, cx.keyboard_mapper().as_ref())
        .map_err(|_| format!("“{key}” is not a valid key"))
}

/// Add the user's bindings. Invalid entries are skipped and reported through `take_errors`.
pub fn apply_user_keymap(cx: &mut App) {
    let Some(file) = path() else { return };
    let Ok(text) = std::fs::read_to_string(&file) else { return };
    let mut errors = Vec::new();
    let binds = match parse(&text) {
        Ok(b) => b,
        Err(e) => {
            errors.push(format!("keymap.toml: {e}"));
            Vec::new()
        }
    };
    let mut built = Vec::new();
    for b in &binds {
        match build(cx, b) {
            Ok(k) => built.push(k),
            Err(e) => errors.push(format!("keymap.toml: {e}")),
        }
    }
    cx.bind_keys(built);
    if let Ok(mut all) = ERRORS.lock() {
        all.extend(errors);
    }
}

pub const TEMPLATE: &str = r#"# mide key bindings. Each [[bind]] adds or changes one shortcut; this file wins over the defaults.
# Run "Preferences: Reload Keymap" from the command palette (⇧⌘P) after saving.
#
# key:     modifiers and a key, e.g. "cmd-k", "ctrl-shift-up", "cmd-alt-l".
#          Separate several keystrokes with a space for a chord: "cmd-k cmd-d".
# action:  a name from the list below ("editor::DuplicateLine" or just "DuplicateLine"),
#          or "none" to free a key.
# context: optional. "Editor" (the text editor), "Workspace" (anywhere), "Palette".
#
# Editor actions: MoveLeft MoveRight MoveUp MoveDown SelectLeft SelectRight SelectUp SelectDown
#   WordLeft WordRight LineStart LineEnd DocStart DocEnd PageUp PageDown Backspace Delete Newline Tab
#   Undo Redo SelectAll Copy Cut Paste Find FindReplace FindNext FindPrev UseSelectionForFind
#   SelectLine DuplicateLine DeleteLine MoveLineUp MoveLineDown JoinLines IndentLines OutdentLines
#   ToggleComment InsertLineAfter InsertLineBefore AddNextOccurrence SelectAllOccurrences
# Workspace actions: Save SaveAs NewFile NewWindow OpenFolder OpenFolderInNewWindow CloseTab NextTab PrevTab
#   QuickOpen CommandPalette FindInFiles GotoLine ToggleSidebar ToggleTerminal NewTerminal ToggleGit
#   ToggleTheme CloseFolder SendSelectionToTerminal SendPathToTerminal Quit
#
# Examples:
# [[bind]]
# key = "cmd-shift-j"
# action = "editor::JoinLines"
# context = "Editor"
#
# [[bind]]
# key = "cmd-k cmd-t"
# action = "ToggleTheme"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bindings_with_optional_context() {
        let b = parse("[[bind]]\nkey = \"cmd-k\"\naction = \"DuplicateLine\"\ncontext = \"Editor\"\n[[bind]]\nkey=\"cmd-e\"\naction=\"none\"\n").unwrap();
        assert_eq!(b.len(), 2);
        assert_eq!(b[0], Bind { key: "cmd-k".into(), action: "DuplicateLine".into(), context: Some("Editor".into()) });
        assert_eq!(b[1].context, None);
    }

    #[test]
    fn empty_and_template_files_are_fine() {
        assert_eq!(parse("").unwrap(), vec![]);
        assert_eq!(parse(TEMPLATE).unwrap(), vec![]);
    }

    #[test]
    fn malformed_toml_is_an_error_not_a_panic() {
        assert!(parse("[[bind]\nkey =").is_err());
    }
}
