//! The ⌘P / ⌘⇧P overlay: type to fuzzy-filter a list, Enter to choose.

use std::path::PathBuf;

use gpui::{
    actions, div, prelude::*, px, rgb, App, ClickEvent, Context, EventEmitter, FocusHandle, Focusable,
    FontWeight, IntoElement, KeyDownEvent, MouseButton, Render, ScrollWheelEvent, Window,
};
use mide_fuzzy::{highlight_runs, Fuzzy, Kind, Match};

use crate::theme::ui;

actions!(palette, [PaletteConfirm, PaletteCancel, PaletteUp, PaletteDown, PalettePageUp, PalettePageDown]);

const VISIBLE_ROWS: usize = 10;
const ROW_H: f32 = 24.0;
const MAX_RESULTS: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    OpenFolder,
    CloseFolder,
    QuickOpen,
    GotoLine,
    NewFile,
    StageHunk,
    RevertHunk,
    NextChange,
    PrevChange,
    EditKeymap,
    ReloadKeymap,
    NewWindow,
    OpenFolderInNewWindow,
    SendSelection,
    SendPath,
    FindInFiles,
    Save,
    SaveAs,
    ReloadFromDisk,
    CloseTab,
    NextTab,
    PrevTab,
    ToggleGit,
    ToggleTerminal,
    NewTerminal,
    CloseTerminal,
    DockBottom,
    DockLeft,
    DockRight,
    ToggleSidebar,
    ThemeLight,
    ThemeDark,
    ToggleTheme,
    TrustFolder,
    EditGlobalConfig,
    EditProjectConfig,
    ReloadConfig,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    File(PathBuf),
    Command(Command),
    /// Start a terminal session from the profile with this name.
    Terminal(String),
    /// Generic: the n-th item of a caller-defined list (confirmations).
    Index(usize),
    /// Free text typed into an input prompt.
    Text(String),
}

pub struct Item {
    pub label: String,
    pub shortcut: String,
    pub choice: Choice,
}

pub enum PaletteEvent {
    Chosen(Choice),
    Dismissed,
}

pub struct Palette {
    items: Vec<Item>,
    labels: Vec<String>,
    fuzzy: Fuzzy,
    query: String,
    matches: Vec<Match>,
    selected: usize,
    /// First visible row.
    top: usize,
    placeholder: String,
    /// Input prompt: Enter submits what was typed instead of picking an item.
    free_text: bool,
    focus_handle: FocusHandle,
}

impl EventEmitter<PaletteEvent> for Palette {}

impl Focusable for Palette {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Palette {
    pub fn new(items: Vec<Item>, kind: Kind, placeholder: &str, cx: &mut Context<Self>) -> Self {
        let labels = items.iter().map(|i| i.label.clone()).collect();
        let mut p = Self {
            items,
            labels,
            fuzzy: Fuzzy::new(kind),
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
            top: 0,
            placeholder: placeholder.to_owned(),
            free_text: false,
            focus_handle: cx.focus_handle(),
        };
        p.refilter();
        p
    }

    /// A single-line text prompt; emits `Choice::Text` on Enter.
    pub fn input(placeholder: &str, cx: &mut Context<Self>) -> Self {
        let mut p = Self::new(Vec::new(), Kind::Text, placeholder, cx);
        p.free_text = true;
        p
    }

    /// Like `input`, with text already typed (used by Rename).
    pub fn input_with(placeholder: &str, initial: &str, cx: &mut Context<Self>) -> Self {
        let mut p = Self::input(placeholder, cx);
        p.query = initial.to_owned();
        p
    }

    fn refilter(&mut self) {
        self.matches = self.fuzzy.search(&self.labels, &self.query, MAX_RESULTS);
        self.selected = 0;
        self.top = 0;
    }

    fn set_query(&mut self, q: String, cx: &mut Context<Self>) {
        self.query = q;
        self.refilter();
        cx.notify();
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.matches.is_empty() {
            return;
        }
        let last = self.matches.len() as isize - 1;
        // Wrap around like Sublime does.
        let n = self.selected as isize + delta;
        self.selected = if delta.abs() == 1 {
            n.rem_euclid(last + 1) as usize
        } else {
            n.clamp(0, last) as usize
        };
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + VISIBLE_ROWS {
            self.top = self.selected + 1 - VISIBLE_ROWS;
        }
        cx.notify();
    }

    fn choose(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.free_text {
            let text = self.query.trim().to_owned();
            if !text.is_empty() {
                cx.emit(PaletteEvent::Chosen(Choice::Text(text)));
            }
            return;
        }
        if let Some(m) = self.matches.get(ix) {
            cx.emit(PaletteEvent::Chosen(self.items[m.index].choice.clone()));
        }
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let m = &k.modifiers;
        if k.key == "backspace" {
            let mut q = self.query.clone();
            if m.platform {
                q.clear();
            } else if m.alt {
                while q.ends_with(' ') {
                    q.pop();
                }
                while q.chars().last().is_some_and(|c| c != ' ' && c != '/') {
                    q.pop();
                }
            } else {
                q.pop();
            }
            self.set_query(q, cx);
            cx.stop_propagation();
            return;
        }
        if m.platform && k.key == "v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                let line = text.lines().next().unwrap_or("").to_owned();
                self.set_query(format!("{}{line}", self.query), cx);
            }
            cx.stop_propagation();
            return;
        }
        if m.platform || m.control {
            return;
        }
        if let Some(ch) = k.key_char.as_deref().filter(|s| !s.is_empty() && !s.chars().any(char::is_control)) {
            self.set_query(format!("{}{ch}", self.query), cx);
            cx.stop_propagation();
        }
    }

    fn on_scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let rows = (f32::from(e.delta.pixel_delta(px(ROW_H)).y) / ROW_H).round() as isize;
        let max_top = self.matches.len().saturating_sub(VISIBLE_ROWS) as isize;
        self.top = (self.top as isize - rows).clamp(0, max_top) as usize;
        cx.notify();
    }
}

impl Render for Palette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = self.matches.len().min(VISIBLE_ROWS);
        let rows = self.matches.iter().enumerate().skip(self.top).take(VISIBLE_ROWS).map(|(ix, m)| {
            let item = &self.items[m.index];
            let selected = ix == self.selected;
            div()
                .id(("palette-row", ix))
                .h(px(ROW_H))
                .px(px(10.))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .when(selected, |d| d.bg(rgb(ui().pal_selected)))
                .when(!selected, |d| d.hover(|h| h.bg(rgb(ui().pal_hover))))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.choose(ix, cx)))
                .child(
                    div().flex().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().children(
                        highlight_runs(&item.label, &m.indices).into_iter().map(|(text, hit)| {
                            div().when(hit, |d| d.font_weight(FontWeight::BOLD).text_color(rgb(ui().pal_match))).child(text)
                        }),
                    ),
                )
                .child(div().flex_none().text_size(px(11.)).text_color(rgb(ui().pal_muted)).child(item.shortcut.clone()))
        });

        div()
            .id("palette")
            .key_context("Palette")
            .track_focus(&self.focus_handle)
            .w(px(580.))
            .flex()
            .flex_col()
            .bg(rgb(ui().pal_bg))
            .rounded_b(px(6.))
            .overflow_hidden()
            .border_1()
            .border_color(rgb(ui().pal_border))
            .shadow_lg()
            .text_size(px(13.))
            .text_color(rgb(ui().pal_text))
            .on_action(cx.listener(|this, _: &PaletteConfirm, _, cx| this.choose(this.selected, cx)))
            .on_action(cx.listener(|_, _: &PaletteCancel, _, cx| cx.emit(PaletteEvent::Dismissed)))
            .on_action(cx.listener(|this, _: &PaletteUp, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &PaletteDown, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &PalettePageUp, _, cx| this.step(-(VISIBLE_ROWS as isize), cx)))
            .on_action(cx.listener(|this, _: &PalettePageDown, _, cx| this.step(VISIBLE_ROWS as isize, cx)))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            // Clicks inside the palette must not reach the dismiss-on-click backdrop.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .h(px(36.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .bg(rgb(ui().pal_input_bg))
                    .text_color(rgb(ui().pal_text))
                    .text_size(px(14.))
                    .child(if self.query.is_empty() {
                        div().text_color(rgb(ui().pal_placeholder)).child(self.placeholder.clone())
                    } else {
                        div().child(self.query.clone())
                    })
                    .child(div().w(px(1.5)).h(px(17.)).ml(px(1.)).bg(rgb(ui().pal_text))),
            )
            .child(if self.free_text {
                div()
            } else if self.matches.is_empty() {
                div().h(px(ROW_H)).px(px(10.)).flex().items_center().text_color(rgb(ui().pal_muted)).child("No matches")
            } else {
                div().h(px(shown as f32 * ROW_H)).flex().flex_col().children(rows)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_equality_for_dispatch() {
        assert_eq!(Choice::Command(Command::Save), Choice::Command(Command::Save));
        assert_ne!(Choice::Terminal("Shell".into()), Choice::Terminal("Claude".into()));
    }
}
