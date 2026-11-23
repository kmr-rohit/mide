//! The floating Find / Replace bar shown over an editor (⌘F, ⌥⌘F).
//!
//! It owns only the fields and options; the editor that created it runs the
//! searches and shows the results through [`FindBar::set_status`].

use gpui::{
    div, prelude::*, px, rgb, App, ClickEvent, Context, CursorStyle, EventEmitter, FocusHandle, Focusable,
    ClipboardItem, FontWeight, IntoElement, KeyDownEvent, MouseButton, Render, Window, svg,
};
use mide_core::search::Query;

use crate::input::{LineInput, Mods, Outcome};
use crate::theme::ui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindEvent {
    /// The search text or an option changed: search again from where the bar opened.
    QueryChanged,
    Next,
    Prev,
    ReplaceOne,
    ReplaceAll,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Find,
    Replace,
}

pub struct FindBar {
    find: LineInput,
    replace: LineInput,
    active: Field,
    show_replace: bool,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    status: String,
    status_is_error: bool,
    focus_handle: FocusHandle,
}

impl EventEmitter<FindEvent> for FindBar {}

impl Focusable for FindBar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl FindBar {
    pub fn new(query: &Query, show_replace: bool, cx: &mut Context<Self>) -> Self {
        Self {
            find: LineInput::new(&query.text),
            replace: LineInput::default(),
            active: Field::Find,
            show_replace,
            case_sensitive: query.case_sensitive,
            whole_word: query.whole_word,
            regex: query.regex,
            status: String::new(),
            status_is_error: false,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn query(&self) -> Query {
        Query {
            text: self.find.text().to_owned(),
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            regex: self.regex,
        }
    }

    pub fn replacement(&self) -> &str {
        self.replace.text()
    }

    pub fn set_status(&mut self, text: String, is_error: bool, cx: &mut Context<Self>) {
        self.status = text;
        self.status_is_error = is_error;
        cx.notify();
    }

    pub fn set_find_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.find.set(text);
        self.active = Field::Find;
        cx.notify();
    }

    /// Show the replace row (and move there if asked).
    pub fn show_replace(&mut self, show: bool, focus_it: bool, cx: &mut Context<Self>) {
        self.show_replace = show;
        self.active = if show && focus_it { Field::Replace } else { Field::Find };
        cx.notify();
    }

    pub fn focus_find_field(&mut self, cx: &mut Context<Self>) {
        self.active = Field::Find;
        cx.notify();
    }

    fn field_mut(&mut self) -> &mut LineInput {
        match self.active {
            Field::Find => &mut self.find,
            Field::Replace => &mut self.replace,
        }
    }

    fn toggle(&mut self, which: u8, cx: &mut Context<Self>) {
        match which {
            0 => self.case_sensitive = !self.case_sensitive,
            1 => self.whole_word = !self.whole_word,
            _ => self.regex = !self.regex,
        }
        cx.emit(FindEvent::QueryChanged);
        cx.notify();
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let m = &k.modifiers;
        let key = k.key.as_str();
        let mut handled = true;

        if m.platform && m.alt {
            // ⌥⌘C / W / R toggle the options; ⌥⌘↩ replaces everything; ⌥⌘F reveals the replace row.
            match key {
                "c" => self.toggle(0, cx),
                "w" => self.toggle(1, cx),
                "r" => self.toggle(2, cx),
                "enter" => cx.emit(FindEvent::ReplaceAll),
                "f" => self.show_replace(true, true, cx),
                _ => handled = false,
            }
        } else if m.platform {
            match key {
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                        let first_line = text.lines().next().unwrap_or("").to_owned();
                        let was = self.active;
                        if self.field_mut().insert(&first_line) && was == Field::Find {
                            cx.emit(FindEvent::QueryChanged);
                        }
                    }
                }
                "c" | "x" => {
                    // Copy / cut the selection; with nothing selected, copy the whole field.
                    let cut = key == "x";
                    let field = self.field_mut();
                    let text = field.selected_text().unwrap_or_else(|| field.text().to_owned());
                    if !text.is_empty() {
                        let was_selected = field.selection().is_some();
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        if cut && was_selected {
                            let was = self.active;
                            if self.field_mut().delete_selection() && was == Field::Find {
                                cx.emit(FindEvent::QueryChanged);
                            }
                        }
                    }
                }
                "g" if m.shift => cx.emit(FindEvent::Prev),
                "g" => cx.emit(FindEvent::Next),
                "f" => self.focus_find_field(cx),
                "a" | "left" | "right" | "backspace" | "delete" => handled = self.edit_key(key, e, cx),
                "enter" => cx.emit(FindEvent::ReplaceAll),
                _ => handled = false,
            }
        } else {
            match key {
                "escape" => cx.emit(FindEvent::Close),
                "enter" if self.active == Field::Replace => cx.emit(FindEvent::ReplaceOne),
                "enter" if m.shift => cx.emit(FindEvent::Prev),
                "enter" => cx.emit(FindEvent::Next),
                "tab" if self.show_replace => {
                    self.active = if self.active == Field::Find { Field::Replace } else { Field::Find };
                }
                "tab" => {}
                "up" => cx.emit(FindEvent::Prev),
                "down" => cx.emit(FindEvent::Next),
                _ => handled = self.edit_key(key, e, cx),
            }
        }
        if handled {
            cx.stop_propagation();
            cx.notify();
        }
    }

    /// Text editing keys go to whichever field is active.
    fn edit_key(&mut self, key: &str, e: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let m = &e.keystroke.modifiers;
        let mods = Mods { alt: m.alt, cmd: m.platform, ctrl: m.control, shift: m.shift };
        let was = self.active;
        match self.field_mut().key(key, e.keystroke.key_char.as_deref(), mods) {
            Outcome::Ignored => false,
            Outcome::Moved => true,
            Outcome::Changed => {
                if was == Field::Find {
                    cx.emit(FindEvent::QueryChanged);
                }
                true
            }
        }
    }

    fn input_box(&self, id: &'static str, field: Field, placeholder: &str, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let u = ui();
        let input = if field == Field::Find { &self.find } else { &self.replace };
        let active = self.active == field;
        let (before, picked, after, caret_last) = input.segments();
        let mut row = div()
            .id(id)
            .h(px(24.))
            .flex_1()
            .min_w_0()
            .px(px(7.))
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .rounded(px(4.))
            .border_1()
            .border_color(rgb(if active { u.accent } else { u.pal_border }))
            .bg(rgb(u.pal_input_bg))
            .text_color(rgb(u.pal_text))
            .text_size(px(13.))
            .cursor(CursorStyle::IBeam)
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.active = field;
                window.focus(&this.focus_handle);
                cx.notify();
            }));
        if input.is_empty() {
            if active {
                row = row.child(div().w(px(1.5)).h(px(15.)).bg(rgb(u.pal_text)));
            }
            row.child(div().text_color(rgb(u.pal_placeholder)).child(placeholder.to_owned()))
        } else {
            let caret = || div().w(px(1.5)).h(px(15.)).flex_none().bg(rgb(u.pal_text));
            row = row.child(before.to_owned());
            if active && !caret_last {
                row = row.child(caret());
            }
            if !picked.is_empty() {
                row = row.child(div().flex_none().bg(rgb(u.term_selection)).child(picked.to_owned()));
            }
            if active && caret_last {
                row = row.child(caret());
            }
            row.child(after.to_owned())
        }
    }

    fn toggle_button(&self, id: &'static str, label: &'static str, on: bool, which: u8, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let u = ui();
        div()
            .id(id)
            .w(px(26.))
            .h(px(24.))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .when(on, |d| d.bg(rgb(u.accent)).text_color(rgb(u.on_accent)))
            .when(!on, |d| d.text_color(rgb(u.pal_muted)).hover(|h| h.bg(rgb(u.pal_hover))))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle(which, cx)))
            .child(label)
    }

    /// The chevron at the left of the Find row that shows or hides the Replace row.
    fn replace_toggle(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let u = ui();
        let icon = if self.show_replace { "icons/chevron-down.svg" } else { "icons/chevron-right.svg" };
        div()
            .id("toggle-replace")
            .w(px(18.))
            .h(px(24.))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|h| h.bg(rgb(u.pal_hover)))
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                let show = !this.show_replace;
                this.show_replace(show, show, cx);
                window.focus(&this.focus_handle);
            }))
            .child(svg().path(icon).size(px(12.)).text_color(rgb(u.pal_muted)))
    }

    fn icon_button(&self, id: &'static str, label: &'static str, event: FindEvent, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let u = ui();
        div()
            .id(id)
            .w(px(24.))
            .h(px(24.))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .text_size(px(13.))
            .text_color(rgb(u.pal_text))
            .cursor_pointer()
            .hover(|h| h.bg(rgb(u.pal_hover)))
            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| cx.emit(event)))
            .child(label)
    }

    fn text_button(&self, id: &'static str, label: &'static str, event: FindEvent, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let u = ui();
        div()
            .id(id)
            .h(px(24.))
            .px(px(9.))
            .flex()
            .flex_none()
            .items_center()
            .rounded(px(4.))
            .border_1()
            .border_color(rgb(u.btn_border))
            .bg(rgb(u.btn_bg))
            .text_size(px(12.))
            .text_color(rgb(u.pal_text))
            .cursor_pointer()
            .hover(|h| h.bg(rgb(u.btn_hover)))
            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| cx.emit(event)))
            .child(label)
    }
}

impl Render for FindBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let u = ui();
        let status_color = if self.status_is_error { u.danger } else { u.pal_muted };
        div()
            .id("find-bar")
            .key_context("FindBar")
            .track_focus(&self.focus_handle)
            .w(px(440.))
            .p(px(8.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .rounded(px(6.))
            .border_1()
            .border_color(rgb(u.pal_border))
            .bg(rgb(u.pal_bg))
            .shadow_lg()
            .on_key_down(cx.listener(Self::on_key_down))
            // Don't let clicks fall through to the text underneath.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .child(self.replace_toggle(cx))
                    .child(self.input_box("find-input", Field::Find, "Find", cx))
                    .child(self.toggle_button("opt-case", "Aa", self.case_sensitive, 0, cx))
                    .child(self.toggle_button("opt-word", "ab", self.whole_word, 1, cx))
                    .child(self.toggle_button("opt-regex", ".*", self.regex, 2, cx))
                    .child(self.icon_button("find-prev", "↑", FindEvent::Prev, cx))
                    .child(self.icon_button("find-next", "↓", FindEvent::Next, cx))
                    .child(self.icon_button("find-close", "×", FindEvent::Close, cx)),
            )
            .when(self.show_replace, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.))
                        .child(self.input_box("replace-input", Field::Replace, "Replace", cx))
                        .child(self.text_button("replace-one", "Replace", FindEvent::ReplaceOne, cx))
                        .child(self.text_button("replace-all", "All", FindEvent::ReplaceAll, cx)),
                )
            })
            .when(!self.status.is_empty(), |d| {
                d.child(div().px(px(2.)).text_size(px(11.)).text_color(rgb(status_color)).child(self.status.clone()))
            })
    }
}
