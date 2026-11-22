//! App theme: one palette for everything around the editor (sidebar, tabs, status
//! bar, git window, palette, welcome screen, terminal), in light and dark.
//!
//! The text editor deliberately has its own theme (`mide_syntax::Theme`) that
//! does not change with this one.

use std::cell::Cell;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Light,
    Dark,
}

impl Mode {
    pub fn toggled(self) -> Self {
        match self {
            Mode::Light => Mode::Dark,
            Mode::Dark => Mode::Light,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Light => "light",
            Mode::Dark => "dark",
        }
    }
}

/// Every color is `0xRRGGBB`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ui {
    pub mode: Mode,

    // ---- sidebar, tabs, status bar -------------------------------------------------
    pub sidebar_bg: u32,
    pub sidebar_selected: u32,
    pub sidebar_hover: u32,
    pub sidebar_text: u32,
    pub sidebar_dim: u32,
    pub sidebar_header: u32,
    pub icon: u32,
    pub icon_dim: u32,
    pub tabbar_bg: u32,
    pub tab_active_bg: u32,
    pub tab_active_text: u32,
    pub tab_inactive_text: u32,
    pub tab_close_hover_bg: u32,
    pub tab_close_hover_text: u32,
    pub status_bg: u32,
    pub status_text: u32,
    pub status_hover_text: u32,

    // ---- shared ------------------------------------------------------------------------
    /// Divider between the sidebar and the editor.
    pub handle: u32,
    pub accent: u32,
    pub accent_hover: u32,
    pub on_accent: u32,
    /// The editor's own (fixed, dark) background, for placeholders drawn in its place.
    pub editor_bg: u32,
    pub empty_text: u32,

    // ---- welcome screen ---------------------------------------------------------------
    pub welcome_bg: u32,
    pub welcome_title: u32,
    pub welcome_text: u32,
    pub welcome_muted: u32,
    pub welcome_hover: u32,
    pub welcome_icon: u32,

    // ---- terminal panel -------------------------------------------------------------------
    pub strip_bg: u32,
    pub strip_text: u32,
    pub strip_dim: u32,
    pub strip_active_bg: u32,
    pub strip_active_text: u32,
    pub strip_hover: u32,
    pub strip_close_hover: u32,
    pub pane_border: u32,
    pub term_bg: u32,
    pub term_fg: u32,
    pub term_selection: u32,
    pub term_cursor: u32,
    pub term_ansi: [u32; 16],
    pub pill_bg: u32,
    pub pill_text: u32,

    // ---- command palette -----------------------------------------------------------------
    pub pal_bg: u32,
    pub pal_input_bg: u32,
    pub pal_selected: u32,
    pub pal_hover: u32,
    pub pal_text: u32,
    pub pal_muted: u32,
    pub pal_border: u32,
    pub pal_match: u32,
    pub pal_placeholder: u32,

    // ---- git window ----------------------------------------------------------------------------
    pub loc_bg: u32,
    pub list_bg: u32,
    pub detail_bg: u32,
    pub toolbar_bg: u32,
    pub select: u32,
    pub hover: u32,
    pub file_hdr: u32,
    pub hunk_hdr: u32,
    pub removed: u32,
    pub added: u32,
    pub removed_strong: u32,
    pub added_strong: u32,
    pub empty_cell: u32,
    pub context_cell: u32,
    pub text: u32,
    pub muted: u32,
    pub divider: u32,
    pub tag_bg: u32,
    pub tag_text: u32,
    pub local_bg: u32,
    pub remote_bg: u32,
    pub remote_text: u32,
    pub stat_del: u32,
    pub stat_add: u32,
    pub ok_bg: u32,
    pub err_bg: u32,
    pub ok_text: u32,
    pub primary: u32,
    pub danger: u32,
    pub btn_bg: u32,
    pub btn_border: u32,
    pub btn_hover: u32,
    pub sbtn_bg: u32,
    pub sbtn_border: u32,
    pub sbtn_hover: u32,
    pub notice_close_hover: u32,
    pub commitbox_bg: u32,
    pub input_border: u32,
    pub lanes: [u32; 8],
}

const LANES: [u32; 8] = [0x8e5ad7, 0xe04f8a, 0xf0963c, 0x3d8fe0, 0x3fb36b, 0xd2a21f, 0x28a8b8, 0xc4584a];

impl Ui {
    pub fn for_mode(mode: Mode) -> Self {
        match mode {
            Mode::Light => Self::light(),
            Mode::Dark => Self::dark(),
        }
    }

    /// Light chrome, matching the Sublime Text reference screenshots.
    pub fn light() -> Self {
        Self {
            mode: Mode::Light,
            sidebar_bg: 0xecedef,
            sidebar_selected: 0xc8ccd1,
            sidebar_hover: 0xe0e2e5,
            sidebar_text: 0x2b2f33,
            sidebar_dim: 0x9aa0a6,
            sidebar_header: 0x6b7177,
            icon: 0x868b91,
            icon_dim: 0xb4b8bd,
            tabbar_bg: 0x73787e,
            tab_active_bg: 0xdee3e7,
            tab_active_text: 0x2b2f33,
            tab_inactive_text: 0xdfe3e7,
            tab_close_hover_bg: 0xc3c8cd,
            tab_close_hover_text: 0x2b2f33,
            status_bg: 0xc7cad1,
            status_text: 0x3d4349,
            status_hover_text: 0x000000,

            handle: 0xc3c6cb,
            accent: 0x3d8fe0,
            accent_hover: 0x4a9be8,
            on_accent: 0xffffff,
            editor_bg: 0x303942,
            empty_text: 0x7a8691,

            welcome_bg: 0xf3f4f6,
            welcome_title: 0x2b2f33,
            welcome_text: 0x5b6269,
            welcome_muted: 0x7b8187,
            welcome_hover: 0xe3e6ea,
            welcome_icon: 0x868b91,

            strip_bg: 0xe3e5e8,
            strip_text: 0x4a5158,
            strip_dim: 0x6b7177,
            strip_active_bg: 0xffffff,
            strip_active_text: 0x2b2f33,
            strip_hover: 0xd6d9de,
            strip_close_hover: 0xc3c8cd,
            pane_border: 0xc3c6cb,
            term_bg: 0xf7f8f9,
            term_fg: 0x2b2f33,
            term_selection: 0xb9d0ea,
            term_cursor: 0x2b2f33,
            term_ansi: [
                0x2b2f33, 0xc0392b, 0x2e7d4f, 0x8a6100, 0x2b6cb0, 0x8e44ad, 0x17808a, 0x6b7177, //
                0x6b7177, 0xd9432f, 0x2f8a55, 0x9a6b00, 0x2f78c4, 0xa04bbd, 0x1b8d98, 0x4a5158,
            ],
            pill_bg: 0x2b2f33,
            pill_text: 0xffffff,

            pal_bg: 0xe4e5e9,
            pal_input_bg: 0xffffff,
            pal_selected: 0xc7cad1,
            pal_hover: 0xd5d8dd,
            pal_text: 0x2b2f33,
            pal_muted: 0x6b7177,
            pal_border: 0xb4b8be,
            pal_match: 0x1a5fb4,
            pal_placeholder: 0x8a9096,

            loc_bg: 0xdddee0,
            list_bg: 0xecedef,
            detail_bg: 0xf3f3f3,
            toolbar_bg: 0xe3e5e8,
            select: 0xd3e0f0,
            hover: 0xe3e6ea,
            file_hdr: 0xbad4ed,
            hunk_hdr: 0xe3edf7,
            removed: 0xf0c2c2,
            added: 0xc6f0c2,
            removed_strong: 0xe59898,
            added_strong: 0x8fd987,
            empty_cell: 0xe6e6e6,
            context_cell: 0xffffff,
            text: 0x2b2f33,
            muted: 0x6b7177,
            divider: 0xc3c6cb,
            tag_bg: 0xf2b83a,
            tag_text: 0x2b2f33,
            local_bg: 0x3d8fe0,
            remote_bg: 0xcfd3d9,
            remote_text: 0x2b2f33,
            stat_del: 0xf0b4b4,
            stat_add: 0xb4e8ae,
            ok_bg: 0xdff2e1,
            err_bg: 0xf8dcdc,
            ok_text: 0x2f8f4e,
            primary: 0x3d8fe0,
            danger: 0xc4584a,
            btn_bg: 0xf6f7f8,
            btn_border: 0xc3c6cb,
            btn_hover: 0xffffff,
            sbtn_bg: 0xffffff,
            sbtn_border: 0x9aa3ad,
            sbtn_hover: 0xeef3f8,
            notice_close_hover: 0xd2d6da,
            commitbox_bg: 0xe9ebee,
            input_border: 0xb4b8be,
            lanes: LANES,
        }
    }

    /// Dark chrome that sits comfortably around the (always dark) editor.
    pub fn dark() -> Self {
        Self {
            mode: Mode::Dark,
            sidebar_bg: 0x262d33,
            sidebar_selected: 0x3a444d,
            sidebar_hover: 0x2f3840,
            sidebar_text: 0xc9d1d9,
            sidebar_dim: 0x6b7680,
            sidebar_header: 0x8a95a0,
            icon: 0x8a95a0,
            icon_dim: 0x566069,
            tabbar_bg: 0x1f2529,
            tab_active_bg: 0x303942,
            tab_active_text: 0xe6eaee,
            tab_inactive_text: 0x8a95a0,
            tab_close_hover_bg: 0x3a444d,
            tab_close_hover_text: 0xe6eaee,
            status_bg: 0x1b2024,
            status_text: 0x9aa5b0,
            status_hover_text: 0xffffff,

            handle: 0x1b2024,
            accent: 0x3d8fe0,
            accent_hover: 0x4a9be8,
            on_accent: 0xffffff,
            editor_bg: 0x303942,
            empty_text: 0x7a8691,

            welcome_bg: 0x2b333a,
            welcome_title: 0xe6eaee,
            welcome_text: 0x9aa5b0,
            welcome_muted: 0x7f8b97,
            welcome_hover: 0x36414a,
            welcome_icon: 0x8795a1,

            strip_bg: 0x262e35,
            strip_text: 0xaab4be,
            strip_dim: 0x8795a1,
            strip_active_bg: 0x3a444d,
            strip_active_text: 0xffffff,
            strip_hover: 0x2f3840,
            strip_close_hover: 0x55616c,
            pane_border: 0x1f262d,
            term_bg: 0x303942,
            term_fg: 0xd8dee9,
            term_selection: 0x4e5a65,
            term_cursor: 0xf8f8f0,
            term_ansi: [
                0x303942, 0xec5f66, 0x99c794, 0xfac863, 0x6699cc, 0xc594c5, 0x5fb3b3, 0xd8dee9, //
                0x7f8b97, 0xf97b58, 0xaddb67, 0xffd479, 0x82aaff, 0xd1a3f7, 0x7fdbca, 0xffffff,
            ],
            pill_bg: 0xd8dee9,
            pill_text: 0x20262b,

            pal_bg: 0x2a3138,
            pal_input_bg: 0x20262b,
            pal_selected: 0x3a4a5c,
            pal_hover: 0x343e47,
            pal_text: 0xd0d7de,
            pal_muted: 0x8b96a1,
            pal_border: 0x3f4a54,
            pal_match: 0x6cb0ff,
            pal_placeholder: 0x7a8691,

            loc_bg: 0x22282d,
            list_bg: 0x262d33,
            detail_bg: 0x2b333a,
            toolbar_bg: 0x20262b,
            select: 0x34506b,
            hover: 0x2f3840,
            file_hdr: 0x34495e,
            hunk_hdr: 0x2f3b46,
            removed: 0x5a2d33,
            added: 0x2d4a35,
            removed_strong: 0x8a3b43,
            added_strong: 0x3f7a4f,
            empty_cell: 0x252b31,
            context_cell: 0x2b333a,
            text: 0xd0d7de,
            muted: 0x8b96a1,
            divider: 0x3a444d,
            tag_bg: 0xc99a2e,
            tag_text: 0x1a1d20,
            local_bg: 0x3d8fe0,
            remote_bg: 0x3f4a54,
            remote_text: 0xd0d7de,
            stat_del: 0x7a3a40,
            stat_add: 0x36643f,
            ok_bg: 0x234d2f,
            err_bg: 0x5a2a2a,
            ok_text: 0x5cc47f,
            primary: 0x3d8fe0,
            danger: 0xe0685a,
            btn_bg: 0x323b44,
            btn_border: 0x465260,
            btn_hover: 0x3b4650,
            sbtn_bg: 0x2b333a,
            sbtn_border: 0x566270,
            sbtn_hover: 0x36424d,
            notice_close_hover: 0x4a555f,
            commitbox_bg: 0x2f373f,
            input_border: 0x4a5560,
            lanes: LANES,
        }
    }
}

thread_local! {
    static CURRENT: Cell<Ui> = Cell::new(Ui::light());
}

/// The active theme. The UI is single-threaded, so this is a cheap thread-local.
pub fn ui() -> Ui {
    CURRENT.with(Cell::get)
}

pub fn set(mode: Mode) {
    CURRENT.with(|c| c.set(Ui::for_mode(mode)));
}

pub fn mode() -> Mode {
    ui().mode
}

/// The colors the terminal emulator resolves ANSI/default colors with.
pub fn terminal_palette() -> mide_term::Palette {
    let u = ui();
    let rgb = |v: u32| mide_term::Rgb8::hex(v);
    mide_term::Palette { fg: rgb(u.term_fg), bg: rgb(u.term_bg), ansi: u.term_ansi.map(rgb) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lin(c: u8) -> f64 {
        let c = c as f64 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    fn luminance(v: u32) -> f64 {
        0.2126 * lin((v >> 16) as u8) + 0.7152 * lin((v >> 8) as u8) + 0.0722 * lin(v as u8)
    }

    /// WCAG contrast ratio, 1.0 (identical) to 21.0 (black on white).
    fn contrast(a: u32, b: u32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    fn check(name: &str, u: &Ui, pairs: &[(&str, u32, u32, f64)]) {
        for (what, fg, bg, min) in pairs {
            let r = contrast(*fg, *bg);
            assert!(r >= *min, "{name} theme: {what} contrast {r:.2} is below {min} ({fg:06x} on {bg:06x})");
        }
        let _ = u;
    }

    fn readability(u: &Ui) -> Vec<(&'static str, u32, u32, f64)> {
        vec![
            ("sidebar text", u.sidebar_text, u.sidebar_bg, 7.0),
            ("sidebar header", u.sidebar_header, u.sidebar_bg, 3.5),
            ("sidebar dim (gitignored)", u.sidebar_dim, u.sidebar_bg, 2.0),
            ("sidebar icon", u.icon, u.sidebar_bg, 2.5),
            ("active tab", u.tab_active_text, u.tab_active_bg, 7.0),
            ("inactive tab", u.tab_inactive_text, u.tabbar_bg, 3.0),
            ("status bar", u.status_text, u.status_bg, 5.0),
            ("status bar hover", u.status_hover_text, u.status_bg, 7.0),
            ("welcome title", u.welcome_title, u.welcome_bg, 7.0),
            ("welcome text", u.welcome_text, u.welcome_bg, 4.5),
            ("welcome muted", u.welcome_muted, u.welcome_bg, 3.0),
            ("terminal strip", u.strip_text, u.strip_bg, 4.5),
            ("terminal strip dim", u.strip_dim, u.strip_bg, 3.0),
            ("terminal active tab", u.strip_active_text, u.strip_active_bg, 7.0),
            ("terminal text", u.term_fg, u.term_bg, 7.0),
            ("scrolled pill", u.pill_text, u.pill_bg, 7.0),
            ("palette text", u.pal_text, u.pal_bg, 7.0),
            ("palette muted", u.pal_muted, u.pal_bg, 3.5),
            ("palette input", u.pal_text, u.pal_input_bg, 7.0),
            ("palette placeholder", u.pal_placeholder, u.pal_input_bg, 2.0),
            ("palette match on selected", u.pal_match, u.pal_selected, 3.0),
            ("palette text on selected", u.pal_text, u.pal_selected, 5.0),
            ("git text on list", u.text, u.list_bg, 7.0),
            ("git muted on list", u.muted, u.list_bg, 3.5),
            ("git text on detail", u.text, u.detail_bg, 7.0),
            ("git muted on detail", u.muted, u.detail_bg, 3.5),
            ("git text on locations", u.text, u.loc_bg, 7.0),
            ("git text on selected row", u.text, u.select, 5.0),
            ("git text on removed", u.text, u.removed, 4.5),
            ("git text on added", u.text, u.added, 4.5),
            ("git text on removed highlight", u.text, u.removed_strong, 3.5),
            ("git text on added highlight", u.text, u.added_strong, 3.5),
            ("git text on file header", u.text, u.file_hdr, 4.5),
            ("git muted on hunk header", u.muted, u.hunk_hdr, 3.0),
            ("git context lines", u.text, u.context_cell, 7.0),
            ("git toolbar text", u.text, u.toolbar_bg, 7.0),
            ("git button", u.text, u.btn_bg, 7.0),
            ("git small button", u.text, u.sbtn_bg, 7.0),
            ("git commit box label", u.muted, u.commitbox_bg, 3.5),
            ("git ok banner", u.text, u.ok_bg, 5.0),
            ("git error banner", u.text, u.err_bg, 5.0),
            ("git tag badge", u.tag_text, u.tag_bg, 4.5),
            ("git remote badge", u.remote_text, u.remote_bg, 4.5),
            ("white on accent", u.on_accent, u.accent, 3.0),
            ("danger on small button", u.danger, u.sbtn_bg, 3.0),
            ("ahead count", u.ok_text, u.toolbar_bg, 3.0),
            ("behind count", u.danger, u.toolbar_bg, 3.0),
        ]
    }

    #[test]
    fn light_theme_is_readable() {
        let u = Ui::light();
        check("light", &u, &readability(&u));
        // Terminal colors against the terminal background (bright white is meant to be
        // the lightest color, so it is exempt on a light background).
        let mut pairs = Vec::new();
        for i in (0..16).filter(|i| *i != 15) {
            pairs.push(("terminal ansi", u.term_ansi[i], u.term_bg, if i == 8 || i == 7 { 3.0 } else { 3.5 }));
        }
        check("light", &u, &pairs);
    }

    #[test]
    fn dark_theme_is_readable() {
        let u = Ui::dark();
        check("dark", &u, &readability(&u));
        // Black is meant to blend into a dark background, so it is exempt there.
        let mut pairs = Vec::new();
        for i in 1..16 {
            pairs.push(("terminal ansi", u.term_ansi[i], u.term_bg, if i == 8 { 2.5 } else { 3.5 }));
        }
        check("dark", &u, &pairs);
    }

    #[test]
    fn themes_are_genuinely_different() {
        let (l, d) = (Ui::light(), Ui::dark());
        assert_ne!(l, d);
        for (what, a, b) in [
            ("sidebar", l.sidebar_bg, d.sidebar_bg),
            ("git list", l.list_bg, d.list_bg),
            ("terminal", l.term_bg, d.term_bg),
            ("palette", l.pal_bg, d.pal_bg),
            ("welcome", l.welcome_bg, d.welcome_bg),
        ] {
            assert!(luminance(a) > luminance(b) + 0.15, "{what}: light should be clearly lighter than dark");
        }
    }

    #[test]
    fn terminal_and_app_move_together() {
        // The terminal must follow the app theme: light app -> light terminal.
        assert!(luminance(Ui::light().term_bg) > 0.7);
        assert!(luminance(Ui::dark().term_bg) < 0.1);
        set(Mode::Dark);
        assert_eq!(terminal_palette().bg, mide_term::Rgb8::hex(Ui::dark().term_bg));
        set(Mode::Light);
        assert_eq!(terminal_palette().bg, mide_term::Rgb8::hex(Ui::light().term_bg));
        assert_eq!(mode(), Mode::Light);
    }

    #[test]
    fn mode_serializes_lowercase_and_toggles() {
        assert_eq!(Mode::Light.toggled(), Mode::Dark);
        assert_eq!(Mode::Dark.toggled().toggled(), Mode::Dark);
        assert_eq!(toml::to_string(&std::collections::BTreeMap::from([("theme", Mode::Dark)])).unwrap().trim(), "theme = \"dark\"");
    }
}
