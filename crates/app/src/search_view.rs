//! Find in files (⇧⌘F): type a pattern, see every match in the project, Enter opens it.
//!
//! The search runs on a background thread and streams results in, so typing never waits
//! for it. A newer keystroke cancels the older search.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    div, prelude::*, px, rgb, App, ClickEvent, Context, EventEmitter, FocusHandle, Focusable, FontWeight,
    IntoElement, KeyDownEvent, MouseButton, Render, ScrollWheelEvent, Window,
};
use mide_core::search::Query;
use mide_files::{Hit, Summary};

use crate::palette::{PaletteCancel, PaletteConfirm, PaletteDown, PalettePageDown, PalettePageUp, PaletteUp};
use crate::theme::ui;

const VISIBLE_ROWS: usize = 14;
const ROW_H: f32 = 22.0;
/// Most results listed; past this the list says so.
const LIMIT: usize = 5000;
const DEBOUNCE: Duration = Duration::from_millis(150);

pub enum SearchEvent {
    /// Open this file at (0-based) line and column.
    Open { path: PathBuf, line: usize, col: usize },
    Dismissed,
}

#[derive(Default)]
enum Status {
    #[default]
    Idle,
    Searching,
    Done(Summary),
    Error(String),
}

pub struct SearchView {
    root: PathBuf,
    query: Query,
    hits: Vec<Hit>,
    status: Status,
    selected: usize,
    top: usize,
    generation: u64,
    cancel: Arc<AtomicBool>,
    focus_handle: FocusHandle,
}

impl EventEmitter<SearchEvent> for SearchView {}

impl Focusable for SearchView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SearchView {
    pub fn new(root: PathBuf, initial: &str, cx: &mut Context<Self>) -> Self {
        let mut v = Self {
            root,
            query: Query { text: initial.to_owned(), ..Default::default() },
            hits: Vec::new(),
            status: Status::Idle,
            selected: 0,
            top: 0,
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            focus_handle: cx.focus_handle(),
        };
        v.restart(cx);
        v
    }

    /// Cancel the running search and start one for the current query, after a short pause.
    fn restart(&mut self, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        self.generation += 1;
        self.hits.clear();
        self.selected = 0;
        self.top = 0;
        if self.query.text.is_empty() {
            self.status = Status::Idle;
            cx.notify();
            return;
        }
        self.status = Status::Searching;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        let generation = self.generation;
        let (root, query) = (self.root.clone(), self.query.clone());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let (tx, rx) = async_channel::unbounded::<Result<Vec<Hit>, Result<Summary, String>>>();
            let flag = cancel.clone();
            std::thread::spawn(move || {
                let batch_tx = tx.clone();
                let done = mide_files::search(&root, &query, LIMIT, &flag, |h| {
                    let _ = batch_tx.send_blocking(Ok(h));
                });
                let _ = tx.send_blocking(Err(done));
            });
            while let Ok(msg) = rx.recv().await {
                let alive = this
                    .update(cx, |v, cx| {
                        if v.generation != generation {
                            return false;
                        }
                        match msg {
                            Ok(batch) => v.hits.extend(batch),
                            Err(Ok(summary)) => v.status = Status::Done(summary),
                            Err(Err(e)) => v.status = Status::Error(e),
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    cancel.store(true, Ordering::Relaxed);
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn edit_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.query.text = text;
        self.restart(cx);
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.hits.is_empty() {
            return;
        }
        let last = self.hits.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + VISIBLE_ROWS {
            self.top = self.selected + 1 - VISIBLE_ROWS;
        }
        cx.notify();
    }

    fn choose(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(h) = self.hits.get(ix) {
            cx.emit(SearchEvent::Open { path: h.path.clone(), line: h.line, col: h.col });
        }
    }

    fn toggle(&mut self, which: char, cx: &mut Context<Self>) {
        match which {
            'c' => self.query.case_sensitive = !self.query.case_sensitive,
            'r' => self.query.regex = !self.query.regex,
            _ => self.query.whole_word = !self.query.whole_word,
        }
        self.restart(cx);
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let m = &k.modifiers;
        if m.platform && m.alt {
            if matches!(k.key.as_str(), "c" | "r" | "w") {
                self.toggle(k.key.chars().next().unwrap_or('c'), cx);
                cx.stop_propagation();
            }
            return;
        }
        if k.key == "backspace" {
            let mut q = self.query.text.clone();
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
            self.edit_text(q, cx);
            cx.stop_propagation();
            return;
        }
        if m.platform && k.key == "v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                let line = text.lines().next().unwrap_or("").to_owned();
                self.edit_text(format!("{}{line}", self.query.text), cx);
            }
            cx.stop_propagation();
            return;
        }
        if m.platform || m.control {
            return;
        }
        if let Some(ch) = k.key_char.as_deref().filter(|s| !s.is_empty() && !s.chars().any(char::is_control)) {
            self.edit_text(format!("{}{ch}", self.query.text), cx);
            cx.stop_propagation();
        }
    }

    fn on_scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let rows = (f32::from(e.delta.pixel_delta(px(ROW_H)).y) / ROW_H).round() as isize;
        let max_top = self.hits.len().saturating_sub(VISIBLE_ROWS) as isize;
        self.top = (self.top as isize - rows).clamp(0, max_top) as usize;
        cx.notify();
    }

    fn status_line(&self) -> String {
        match &self.status {
            Status::Idle => "Type to search the whole project".to_owned(),
            Status::Searching => format!("Searching… {} so far", self.hits.len()),
            Status::Done(s) if s.hits == 0 => "No results".to_owned(),
            Status::Done(s) => format!(
                "{}{} result{} in {} file{}",
                s.hits,
                if s.truncated { "+" } else { "" },
                if s.hits == 1 { "" } else { "s" },
                s.files_with_hits,
                if s.files_with_hits == 1 { "" } else { "s" }
            ),
            Status::Error(e) => format!("Invalid pattern: {e}"),
        }
    }
}

fn chip(label: &'static str, on: bool, id: &'static str, tip: char, cx: &mut Context<SearchView>) -> impl IntoElement {
    div()
        .id(id)
        .px(px(6.))
        .h(px(18.))
        .flex()
        .items_center()
        .rounded(px(3.))
        .cursor_pointer()
        .text_size(px(11.))
        .when(on, |d| d.bg(rgb(ui().pal_selected)).text_color(rgb(ui().pal_match)).font_weight(FontWeight::BOLD))
        .when(!on, |d| d.text_color(rgb(ui().pal_muted)).hover(|h| h.bg(rgb(ui().pal_hover))))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle(tip, cx)))
        .child(label)
}

impl Render for SearchView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = self.hits.len().min(VISIBLE_ROWS);
        let rows = self.hits.iter().enumerate().skip(self.top).take(VISIBLE_ROWS).map(|(ix, h)| {
            let selected = ix == self.selected;
            let chars: Vec<char> = h.text.chars().collect();
            let (a, b) = (h.start.min(chars.len()), h.end.min(chars.len()));
            let before: String = chars[..a].iter().collect();
            let hit: String = chars[a..b].iter().collect();
            let after: String = chars[b..].iter().collect();
            div()
                .id(("search-row", ix))
                .h(px(ROW_H))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(10.))
                .when(selected, |d| d.bg(rgb(ui().pal_selected)))
                .when(!selected, |d| d.hover(|h| h.bg(rgb(ui().pal_hover))))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.choose(ix, cx)))
                .child(
                    div()
                        .flex_none()
                        .w(px(230.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(12.))
                        .text_color(rgb(ui().pal_muted))
                        .child(format!("{}:{}", h.rel, h.line + 1)),
                )
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .font_family("Monaco")
                        .text_size(px(12.))
                        .child(before)
                        .child(div().font_weight(FontWeight::BOLD).text_color(rgb(ui().pal_match)).child(hit))
                        .child(after),
                )
        }).collect::<Vec<_>>();
        let q = &self.query;
        div()
            .id("search")
            .key_context("Palette")
            .track_focus(&self.focus_handle)
            .w(px(820.))
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
            .on_action(cx.listener(|_, _: &PaletteCancel, _, cx| cx.emit(SearchEvent::Dismissed)))
            .on_action(cx.listener(|this, _: &PaletteUp, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &PaletteDown, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &PalettePageUp, _, cx| this.step(-(VISIBLE_ROWS as isize), cx)))
            .on_action(cx.listener(|this, _: &PalettePageDown, _, cx| this.step(VISIBLE_ROWS as isize, cx)))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .h(px(36.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .bg(rgb(ui().pal_input_bg))
                    .text_size(px(14.))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .items_center()
                            .child(if q.text.is_empty() {
                                div().text_color(rgb(ui().pal_placeholder)).child("Find in files…")
                            } else {
                                div().child(q.text.clone())
                            })
                            .child(div().w(px(1.5)).h(px(17.)).ml(px(1.)).bg(rgb(ui().pal_text))),
                    )
                    .child(chip("Aa", q.case_sensitive, "opt-case", 'c', cx))
                    .child(chip("W", q.whole_word, "opt-word", 'w', cx))
                    .child(chip(".*", q.regex, "opt-regex", 'r', cx)),
            )
            .child(div().h(px(shown as f32 * ROW_H)).flex().flex_col().children(rows))
            .child(
                div()
                    .h(px(22.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(11.))
                    .text_color(rgb(ui().pal_muted))
                    .child(self.status_line())
                    .child("⌥⌘C case · ⌥⌘W word · ⌥⌘R regex"),
            )
    }
}
