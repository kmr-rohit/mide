mod assets;
mod config;
mod editor;
mod find_bar;
mod fonts;
mod git_view;
mod input;
mod keys;
mod keymap;
mod layout;
mod palette;
mod profile;
mod perf;
mod recent;
mod search_view;
mod session;
mod terminal_view;
mod theme;
mod trust;
mod workspace;

use std::path::PathBuf;

use gpui::{
    px, size, App, AppContext, Application, Bounds, KeyBinding, Menu, MenuItem, OsAction, TitlebarOptions,
    WindowBounds, WindowOptions,
};

use editor::*;
use git_view::*;
use palette::*;
use workspace::*;

/// Create the editor window. Returns `None` if the system refuses.
pub(crate) fn open_main_window(cx: &mut App, root: Option<PathBuf>, initial: Option<PathBuf>) -> Option<gpui::WindowHandle<Workspace>> {
    let bounds = Bounds::centered(None, size(px(1200.), px(780.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("mide".into()), ..Default::default() }),
            ..Default::default()
        },
        move |window, cx| {
            perf::mark("window created");
            cx.new(|cx| Workspace::new(root.clone(), initial.clone(), window, cx))
        },
    )
    .ok()
}

fn main() {
    perf::init();

    // `mide [path]`: a folder opens as the project, a file opens its folder + the
    // file. With no argument the window starts on the welcome screen.
    let arg = std::env::args_os().nth(1).map(PathBuf::from);
    let (root, initial) = match arg {
        Some(p) if p.is_file() => {
            let abs = p.canonicalize().unwrap_or(p);
            (abs.parent().map(PathBuf::from), Some(abs))
        }
        Some(p) => (Some(p), None),
        None => (None, None),
    };

    // Finder, `open -a mide <path>` and the `mide` command deliver paths as open-URL
    // events; they may arrive before the window exists, so they queue in a channel.
    let (url_tx, url_rx) = async_channel::unbounded::<Vec<String>>();
    let app = Application::new().with_assets(assets::Assets);
    app.on_open_urls(move |urls| {
        let _ = url_tx.try_send(urls);
    });

    // Clicking the Dock icon after the window was closed brings a window back, on the
    // most recent project (its tabs return through session restore).
    app.on_reopen(|cx| {
        if cx.windows().is_empty() {
            let last = recent::load().into_iter().next();
            open_main_window(cx, last, None);
        }
        cx.activate(true);
    });

    app.run(move |cx: &mut App| {
        bind_keys(cx);
        keymap::apply_user_keymap(cx);
        cx.on_action(|_: &NewWindow, cx| {
            open_main_window(cx, None, None);
        });
        set_menus(cx);
        open_main_window(cx, root.clone(), initial.clone());
        // Paths from Finder / `mide <path>`. The window may have been closed (the app keeps
        // running, like any Mac app), so find it again or open a fresh one each time.
        cx.spawn(async move |cx| {
            while let Ok(urls) = url_rx.recv().await {
                let _ = cx.update(|cx| {
                    let handle = cx
                        .windows()
                        .into_iter()
                        .find_map(|w| w.downcast::<Workspace>())
                        .or_else(|| open_main_window(cx, None, None));
                    if let Some(h) = handle {
                        let _ = h.update(cx, |ws, window, cx| ws.open_urls(urls, window, cx));
                        let _ = h.update(cx, |_, window, _| window.activate_window());
                    }
                });
            }
        })
        .detach();
        cx.activate(true);
    });
}

/// The macOS menu bar. Shortcuts shown here come from the key bindings.
pub(crate) fn set_menus(cx: &mut App) {
    cx.set_menus(vec![
        Menu {
            name: "mide".into(),
            items: vec![MenuItem::action("Quit mide", Quit)],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("Open Folder…", OpenFolder),
                MenuItem::action("Close Folder", CloseFolder),
                MenuItem::action("Go to File…", QuickOpen),
                MenuItem::separator(),
                MenuItem::action("New File", NewFile),
                MenuItem::action("New Window", NewWindow),
                MenuItem::action("Open Folder in New Window…", OpenFolderInNewWindow),
                MenuItem::action("Save", Save),
                MenuItem::action("Save As…", SaveAs),
                MenuItem::action("Close Tab", CloseTab),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Undo", Undo, OsAction::Undo),
                MenuItem::os_action("Redo", Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", Cut, OsAction::Cut),
                MenuItem::os_action("Copy", Copy, OsAction::Copy),
                MenuItem::os_action("Paste", Paste, OsAction::Paste),
                MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
                MenuItem::separator(),
                MenuItem::action("Find…", Find),
                MenuItem::action("Find in Files…", FindInFiles),
                MenuItem::action("Find Next", FindNext),
                MenuItem::action("Find Previous", FindPrev),
                MenuItem::action("Replace…", FindReplace),
                MenuItem::action("Use Selection for Find", UseSelectionForFind),
                MenuItem::action("Go to Line…", GotoLine),
                MenuItem::separator(),
                MenuItem::action("Select Line", SelectLine),
                MenuItem::action("Add Next Occurrence", AddNextOccurrence),
                MenuItem::action("Select All Occurrences", SelectAllOccurrences),
                MenuItem::action("Duplicate Line", DuplicateLine),
                MenuItem::action("Delete Line", DeleteLine),
                MenuItem::action("Move Line Up", MoveLineUp),
                MenuItem::action("Move Line Down", MoveLineDown),
                MenuItem::action("Join Lines", JoinLines),
                MenuItem::action("Indent", IndentLines),
                MenuItem::action("Outdent", OutdentLines),
                MenuItem::action("Toggle Comment", ToggleComment),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Command Palette…", CommandPalette),
                MenuItem::separator(),
                MenuItem::action("Toggle Sidebar", ToggleSidebar),
                MenuItem::action("Toggle Light/Dark Theme", ToggleTheme),
                MenuItem::action("Toggle Terminal", ToggleTerminal),
                MenuItem::action("New Terminal Session…", NewTerminal),
                MenuItem::action("Git History", ToggleGit),
                MenuItem::separator(),
                MenuItem::action("Next Tab", NextTab),
                MenuItem::action("Previous Tab", PrevTab),
            ],
        },
    ]);
}

pub(crate) fn bind_keys(cx: &mut App) {
    let ed = Some("Editor");
    cx.bind_keys([
        KeyBinding::new("left", MoveLeft, ed),
        KeyBinding::new("right", MoveRight, ed),
        KeyBinding::new("up", MoveUp, ed),
        KeyBinding::new("down", MoveDown, ed),
        KeyBinding::new("shift-left", SelectLeft, ed),
        KeyBinding::new("shift-right", SelectRight, ed),
        KeyBinding::new("shift-up", SelectUp, ed),
        KeyBinding::new("shift-down", SelectDown, ed),
        KeyBinding::new("alt-left", WordLeft, ed),
        KeyBinding::new("alt-right", WordRight, ed),
        KeyBinding::new("alt-shift-left", SelectWordLeft, ed),
        KeyBinding::new("alt-shift-right", SelectWordRight, ed),
        KeyBinding::new("cmd-left", LineStart, ed),
        KeyBinding::new("home", LineStart, ed),
        KeyBinding::new("cmd-right", LineEnd, ed),
        KeyBinding::new("end", LineEnd, ed),
        KeyBinding::new("cmd-shift-left", SelectLineStart, ed),
        KeyBinding::new("cmd-shift-right", SelectLineEnd, ed),
        KeyBinding::new("cmd-up", DocStart, ed),
        KeyBinding::new("cmd-down", DocEnd, ed),
        KeyBinding::new("cmd-shift-up", SelectDocStart, ed),
        KeyBinding::new("cmd-shift-down", SelectDocEnd, ed),
        KeyBinding::new("pageup", PageUp, ed),
        KeyBinding::new("pagedown", PageDown, ed),
        KeyBinding::new("shift-pageup", SelectPageUp, ed),
        KeyBinding::new("shift-pagedown", SelectPageDown, ed),
        KeyBinding::new("backspace", Backspace, ed),
        KeyBinding::new("delete", Delete, ed),
        KeyBinding::new("alt-backspace", DeleteWordBackward, ed),
        KeyBinding::new("enter", Newline, ed),
        KeyBinding::new("tab", Tab, ed),
        KeyBinding::new("cmd-z", Undo, ed),
        KeyBinding::new("cmd-shift-z", Redo, ed),
        KeyBinding::new("cmd-y", Redo, ed),
        KeyBinding::new("cmd-a", SelectAll, ed),
        KeyBinding::new("cmd-c", Copy, ed),
        KeyBinding::new("cmd-x", Cut, ed),
        KeyBinding::new("cmd-v", Paste, ed),
        KeyBinding::new("cmd-f", Find, ed),
        KeyBinding::new("cmd-alt-f", FindReplace, ed),
        KeyBinding::new("cmd-g", FindNext, ed),
        KeyBinding::new("cmd-shift-g", FindPrev, ed),
        KeyBinding::new("cmd-e", UseSelectionForFind, ed),
        KeyBinding::new("cmd-l", SelectLine, ed),
        KeyBinding::new("cmd-shift-d", DuplicateLine, ed),
        KeyBinding::new("ctrl-shift-k", DeleteLine, ed),
        KeyBinding::new("ctrl-cmd-up", MoveLineUp, ed),
        KeyBinding::new("ctrl-cmd-down", MoveLineDown, ed),
        KeyBinding::new("cmd-j", JoinLines, ed),
        KeyBinding::new("cmd-]", IndentLines, ed),
        KeyBinding::new("cmd-[", OutdentLines, ed),
        KeyBinding::new("shift-tab", OutdentLines, ed),
        KeyBinding::new("cmd-/", ToggleComment, ed),
        // ⌘↩ is "commit" inside the git window's message box, so not there.
        KeyBinding::new("cmd-enter", InsertLineAfter, Some("Editor && !CommitMessage")),
        KeyBinding::new("cmd-shift-enter", InsertLineBefore, Some("Editor && !CommitMessage")),
        KeyBinding::new("cmd-d", AddNextOccurrence, ed),
        KeyBinding::new("ctrl-cmd-g", SelectAllOccurrences, ed),
        KeyBinding::new("cmd-s", Save, None),
        KeyBinding::new("cmd-alt-down", NextChange, ed),
        KeyBinding::new("cmd-alt-up", PrevChange, ed),
        KeyBinding::new("cmd-shift-n", NewWindow, None),
        KeyBinding::new("cmd-alt-o", OpenFolderInNewWindow, None),
        KeyBinding::new("cmd-shift-f", FindInFiles, None),
        KeyBinding::new("cmd-shift-s", SaveAs, None),
        KeyBinding::new("cmd-n", NewFile, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("cmd-shift-]", NextTab, None),
        KeyBinding::new("cmd-shift-[", PrevTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PrevTab, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("ctrl-shift-g", ToggleGit, None),
        KeyBinding::new("ctrl-g", GotoLine, None),
        KeyBinding::new("cmd-o", OpenFolder, None),
        KeyBinding::new("cmd-p", QuickOpen, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("cmd-alt-t", ToggleTheme, None),
        KeyBinding::new("cmd-shift-p", CommandPalette, None),
        KeyBinding::new("ctrl-`", ToggleTerminal, None),
        KeyBinding::new("cmd-shift-t", NewTerminal, None),
        KeyBinding::new("enter", PaletteConfirm, Some("Palette")),
        KeyBinding::new("escape", PaletteCancel, Some("Palette")),
        KeyBinding::new("up", PaletteUp, Some("Palette")),
        KeyBinding::new("down", PaletteDown, Some("Palette")),
        KeyBinding::new("ctrl-p", PaletteUp, Some("Palette")),
        KeyBinding::new("ctrl-n", PaletteDown, Some("Palette")),
        KeyBinding::new("pageup", PalettePageUp, Some("Palette")),
        KeyBinding::new("pagedown", PalettePageDown, Some("Palette")),
        KeyBinding::new("escape", CloseGit, Some("GitView")),
        KeyBinding::new("cmd-enter", DoCommit, Some("CommitMessage")),
        KeyBinding::new("up", PrevCommit, Some("GitView")),
        KeyBinding::new("down", NextCommit, Some("GitView")),
        KeyBinding::new("pageup", PagePrevCommit, Some("GitView")),
        KeyBinding::new("pagedown", PageNextCommit, Some("GitView")),
        KeyBinding::new("home", FirstCommit, Some("GitView")),
        KeyBinding::new("end", LastCommit, Some("GitView")),
    ]);
}
