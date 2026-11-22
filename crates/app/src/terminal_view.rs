//! One terminal session: paints a `mide_term` snapshot on the GPU and turns
//! keyboard, mouse and scroll input into terminal input.

use std::time::Duration;

use gpui::{
    div, fill, point, prelude::*, px, relative, size, App, Bounds, ClipboardItem, Context, CursorStyle,
    ElementId, Entity, EventEmitter, FocusHandle, Focusable, Font, FontFeatures, FontStyle, FontWeight,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, KeyDownEvent, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, ScrollWheelEvent, Style,
    TextRun, UnderlineStyle, Window,
};
use mide_term::{CursorKind, Rgb8, SelectKind, Size, SnapCell, SpawnOptions, TermEvent, Terminal};

use crate::fonts;
use crate::keys::{self, MouseBtn, MouseKind, Mods};
use crate::theme::{self, ui};

const PAD_X: f32 = 8.0;
const PAD_Y: f32 = 4.0;

pub enum TerminalEvent {
    /// The program ended; `None` means it was killed by a signal.
    Exited(Option<i32>),
    /// The window title or output changed in a way the tab strip may show.
    TitleChanged,
}

#[derive(Clone, Copy)]
struct Geometry {
    origin: Point<Pixels>,
    cell_w: f32,
}

pub struct TerminalView {
    term: Terminal,
    focus_handle: FocusHandle,
    geometry: Option<Geometry>,
    selecting: bool,
    scroll_accum: f32,
    /// Button currently held while reporting mouse events to the program.
    held: Option<MouseBtn>,
    last_cell: Option<(usize, usize)>,
    /// Lines scrolled up into history (drives the "back to bottom" pill).
    scrolled: usize,
    pub exited: Option<Option<i32>>,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn color(c: Rgb8) -> Hsla {
    gpui::rgb(((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32).into()
}

fn font(bold: bool, italic: bool) -> Font {
    Font {
        family: fonts::term_family(),
        features: FontFeatures::default(),
        fallbacks: None,
        weight: if bold { FontWeight::BOLD } else { FontWeight::NORMAL },
        style: if italic { FontStyle::Italic } else { FontStyle::Normal },
    }
}

impl TerminalView {
    /// Start the program and wrap it in a view. Fails if the PTY can't be created.
    pub fn spawn(opts: SpawnOptions, cx: &mut App) -> std::io::Result<Entity<Self>> {
        // The terminal's I/O thread pokes this channel when output arrives; the
        // view sleeps on it, so an idle session costs no CPU at all.
        let (wake_tx, wake_rx) = async_channel::unbounded::<()>();
        let term = Terminal::spawn_with_waker(opts, Size::new(80, 16), move || {
            let _ = wake_tx.try_send(());
        })?;
        Ok(cx.new(|cx: &mut Context<Self>| {
            cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
                while wake_rx.recv().await.is_ok() {
                    // Coalesce a burst of wake-ups into one repaint.
                    while wake_rx.try_recv().is_ok() {}
                    if this.update(cx, |tv, cx| tv.pump(cx)).is_err() {
                        break;
                    }
                    // Cap repaints at ~250/s under heavy output (e.g. `cat bigfile`).
                    cx.background_executor().timer(Duration::from_millis(4)).await;
                }
            })
            .detach();
            Self {
                term,
                focus_handle: cx.focus_handle(),
                geometry: None,
                selecting: false,
                scroll_accum: 0.0,
                held: None,
                last_cell: None,
                scrolled: 0,
                exited: None,
            }
        }))
    }

    pub fn title(&self) -> &str {
        self.term.title()
    }

    /// Drain terminal events; repaint if anything changed.
    fn pump(&mut self, cx: &mut Context<Self>) {
        for ev in self.term.poll_events() {
            match ev {
                TermEvent::Wakeup => cx.notify(),
                TermEvent::Title(_) => {
                    cx.emit(TerminalEvent::TitleChanged);
                    cx.notify();
                }
                TermEvent::ClipboardStore(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                TermEvent::Exited(code) => {
                    self.exited = Some(code);
                    cx.emit(TerminalEvent::Exited(code));
                    cx.notify();
                }
                TermEvent::Bell => {}
            }
        }
    }

    pub fn paste_text(&mut self, text: &str) {
        self.term.scroll_to_bottom();
        self.term.paste(text);
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.term.selection_text().filter(|t| !t.is_empty()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    // ---- input -------------------------------------------------------------------

    fn on_key_down(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        if k.modifiers.platform {
            // ⌘ combos belong to the app, except copy/paste which act on the terminal.
            match k.key.as_str() {
                "c" => {
                    self.copy_selection(cx);
                    cx.stop_propagation();
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                        self.paste_text(&text);
                    }
                    cx.stop_propagation();
                }
                _ => {}
            }
            return;
        }
        // Shift+PageUp/PageDown/Home/End browse scrollback (like every terminal), unless a
        // full-screen program owns the screen and wants those keys itself.
        if k.modifiers.shift && !self.term.alt_screen() {
            let handled = match k.key.as_str() {
                "pageup" => {
                    self.term.scroll_page(true);
                    true
                }
                "pagedown" => {
                    self.term.scroll_page(false);
                    true
                }
                "home" => {
                    self.term.scroll_to_top();
                    true
                }
                "end" => {
                    self.term.scroll_to_bottom();
                    true
                }
                _ => false,
            };
            if handled {
                cx.stop_propagation();
                cx.notify();
                return;
            }
        }
        let mods = Mods { ctrl: k.modifiers.control, alt: k.modifiers.alt, shift: k.modifiers.shift };
        if let Some(bytes) = keys::encode(&k.key, k.key_char.as_deref(), mods, self.term.app_cursor()) {
            self.term.clear_selection();
            self.term.scroll_to_bottom();
            self.term.write(bytes);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn cell_at(&self, pos: Point<Pixels>) -> Option<(usize, usize)> {
        let g = self.geometry?;
        let size = self.term.size();
        let x = f32::from(pos.x - g.origin.x).max(0.0);
        let y = f32::from(pos.y - g.origin.y).max(0.0);
        let col = ((x / g.cell_w) as usize).min(size.cols as usize - 1);
        let row = ((y / fonts::term_line_height()) as usize).min(size.rows as usize - 1);
        Some((row, col))
    }

    fn report(&mut self, btn: MouseBtn, kind: MouseKind, m: &gpui::Modifiers, cell: (usize, usize)) {
        let sgr = self.term.mouse_mode().sgr;
        let mods = Mods { ctrl: m.control, alt: m.alt, shift: m.shift };
        if let Some(bytes) = keys::encode_mouse(btn, kind, mods, cell.1, cell.0, sgr) {
            self.term.write(bytes);
        }
    }

    /// Does the program want mouse events? Holding Shift always selects text
    /// instead, as in other terminals.
    fn program_wants_mouse(&self, shift: bool) -> bool {
        !shift && self.term.mouse_mode().active()
    }

    fn on_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        let Some(cell) = self.cell_at(e.position) else { return };
        if self.program_wants_mouse(e.modifiers.shift) {
            let btn = match e.button {
                MouseButton::Left => MouseBtn::Left,
                MouseButton::Middle => MouseBtn::Middle,
                MouseButton::Right => MouseBtn::Right,
                _ => return,
            };
            self.held = Some(btn);
            self.last_cell = Some(cell);
            self.term.clear_selection();
            self.report(btn, MouseKind::Press, &e.modifiers, cell);
            cx.notify();
            return;
        }
        if e.button != MouseButton::Left {
            return;
        }
        let kind = match e.click_count {
            1 => SelectKind::Simple,
            2 => SelectKind::Word,
            _ => SelectKind::Line,
        };
        self.term.select(kind, cell.0, cell.1);
        self.selecting = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(cell) = self.cell_at(e.position) else { return };
        if self.program_wants_mouse(e.modifiers.shift) {
            let mode = self.term.mouse_mode();
            let wanted = if self.held.is_some() { mode.drag || mode.motion } else { mode.motion };
            if wanted && self.last_cell != Some(cell) {
                self.last_cell = Some(cell);
                let btn = self.held.unwrap_or(MouseBtn::None);
                self.report(btn, MouseKind::Motion, &e.modifiers, cell);
            }
            return;
        }
        if self.selecting {
            self.term.extend_selection(cell.0, cell.1);
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, e: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(btn) = self.held.take() {
            if let Some(cell) = self.cell_at(e.position) {
                self.report(btn, MouseKind::Release, &e.modifiers, cell);
            }
            return;
        }
        if !self.selecting {
            return;
        }
        self.selecting = false;
        // A plain click (nothing dragged) shouldn't leave a one-cell selection behind.
        if self.term.selection_text().is_none_or(|t| t.trim().is_empty()) {
            self.term.clear_selection();
        }
        cx.notify();
    }

    /// The wheel does what each kind of program expects:
    /// * it asked for mouse events (vim, tmux, htop, Claude Code's full-screen UI) -> wheel reports;
    /// * full-screen without mouse events (less, man) -> arrow keys;
    /// * ordinary shell output -> scroll the history.
    fn on_scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_accum += f32::from(e.delta.pixel_delta(px(fonts::term_line_height())).y) / fonts::term_line_height();
        let lines = self.scroll_accum.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_accum -= lines as f32;
        let up = lines > 0;
        let steps = lines.unsigned_abs().min(10);

        if self.program_wants_mouse(e.modifiers.shift) {
            let cell = self.cell_at(e.position).unwrap_or((0, 0));
            let btn = if up { MouseBtn::WheelUp } else { MouseBtn::WheelDown };
            for _ in 0..steps {
                self.report(btn, MouseKind::Press, &e.modifiers, cell);
            }
        } else if self.term.alt_screen() {
            if self.term.alternate_scroll() {
                let key = if up { "up" } else { "down" };
                for _ in 0..steps {
                    if let Some(b) = keys::encode(key, None, Mods::default(), self.term.app_cursor()) {
                        self.term.write(b);
                    }
                }
            }
        } else {
            self.term.scroll(lines);
        }
        cx.notify();
    }
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let u = ui();
        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .size_full()
            .relative()
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(TerminalElement { view: cx.entity() })
            .when(self.scrolled > 0, |d| {
                d.child(
                    div()
                        .id("scrolled-pill")
                        .absolute()
                        .bottom(px(10.))
                        .right(px(18.))
                        .h(px(22.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .rounded(px(11.))
                        .bg(gpui::rgb(u.pill_bg))
                        .text_color(gpui::rgb(u.pill_text))
                        .text_size(px(11.))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                            this.term.scroll_to_bottom();
                            cx.notify();
                        }))
                        .child(format!("↓ {} lines up · click to return", self.scrolled)),
                )
            })
    }
}

// ---- painting ---------------------------------------------------------------------------

struct TerminalElement {
    view: Entity<TerminalView>,
}

struct Segment {
    col: usize,
    text: String,
    fg: Rgb8,
    bold: bool,
    italic: bool,
    underline: bool,
    strikeout: bool,
}

struct Paint {
    bounds: Bounds<Pixels>,
    default_bg: Rgb8,
    cell_w: f32,
    font_size: f32,
    /// Per row: background runs `(start_col, end_col, color)`.
    backgrounds: Vec<Vec<(usize, usize, Rgb8)>>,
    rows: Vec<Vec<Segment>>,
    cursor: Option<(usize, usize, CursorKind, SnapCell)>,
    focused: bool,
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl gpui::Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Paint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Paint {
        let font_size = fonts::term_size();
        let font_id = window.text_system().resolve_font(&font(false, false));
        let cell_w = window.text_system().em_advance(font_id, px(font_size)).map_or(7.8, f32::from);
        let focused = self.view.read(cx).focus_handle.is_focused(window);
        let cols = ((f32::from(bounds.size.width) - 2.0 * PAD_X) / cell_w).floor().max(2.0) as u16;
        let rows = ((f32::from(bounds.size.height) - 2.0 * PAD_Y) / fonts::term_line_height()).floor().max(1.0) as u16;
        let origin = point(bounds.left() + px(PAD_X), bounds.top() + px(PAD_Y));

        let snap = self.view.update(cx, |tv, cx| {
            // The terminal follows the app theme.
            tv.term.set_palette(theme::terminal_palette());
            tv.geometry = Some(Geometry { origin, cell_w });
            let mut size = Size::new(cols, rows);
            size.cell_width = cell_w as u16;
            size.cell_height = fonts::term_line_height() as u16;
            tv.term.resize(size);
            let snap = tv.term.snapshot();
            if tv.scrolled != snap.display_offset {
                tv.scrolled = snap.display_offset;
                cx.notify();
            }
            snap
        });

        let mut backgrounds = Vec::with_capacity(snap.rows);
        let mut seg_rows = Vec::with_capacity(snap.rows);
        for r in 0..snap.rows {
            let mut bgs: Vec<(usize, usize, Rgb8)> = Vec::new();
            let mut segs: Vec<Segment> = Vec::new();
            for c in 0..snap.cols {
                let cell = snap.cell(r, c);
                if cell.style.wide_spacer {
                    continue;
                }
                let bg = if cell.selected { Rgb8::hex(ui().term_selection) } else { cell.bg };
                let width = if cell.style.wide { 2 } else { 1 };
                if bg != snap.default_bg {
                    match bgs.last_mut() {
                        Some(last) if last.1 == c && last.2 == bg => last.1 = c + width,
                        _ => bgs.push((c, c + width, bg)),
                    }
                }
                let ch = if cell.ch == '\0' { ' ' } else { cell.ch };
                let s = &cell.style;
                let blank = ch == ' ' && !s.underline && !s.strikeout;
                if blank {
                    continue;
                }
                match segs.last_mut() {
                    Some(l)
                        if l.fg == cell.fg
                            && l.bold == s.bold
                            && l.italic == s.italic
                            && l.underline == s.underline
                            && l.strikeout == s.strikeout
                            && l.col + l.text.chars().count() == c =>
                    {
                        l.text.push(ch)
                    }
                    _ => segs.push(Segment {
                        col: c,
                        text: ch.to_string(),
                        fg: cell.fg,
                        bold: s.bold,
                        italic: s.italic,
                        underline: s.underline,
                        strikeout: s.strikeout,
                    }),
                }
            }
            backgrounds.push(bgs);
            seg_rows.push(segs);
        }
        let cursor = snap.cursor.map(|c| (c.row, c.col, c.kind, *snap.cell(c.row, c.col)));
        Paint {
            bounds,
            default_bg: snap.default_bg,
            cell_w,
            font_size,
            backgrounds,
            rows: seg_rows,
            cursor,
            focused,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        p: &mut Paint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let origin = point(bounds.left() + px(PAD_X), bounds.top() + px(PAD_Y));
        let lh = px(fonts::term_line_height());
        let x_of = |col: usize| origin.x + px(col as f32 * p.cell_w);
        let y_of = |row: usize| origin.y + px(row as f32 * fonts::term_line_height());

        window.paint_quad(fill(p.bounds, color(p.default_bg)));
        for (r, bgs) in p.backgrounds.iter().enumerate() {
            for &(a, b, c) in bgs {
                window.paint_quad(fill(
                    Bounds::new(point(x_of(a), y_of(r)), size(px((b - a) as f32 * p.cell_w), lh)),
                    color(c),
                ));
            }
        }

        let shape = |window: &mut Window, text: &str, fg: Rgb8, bold: bool, italic: bool, underline: bool, strike: bool| {
            let run = TextRun {
                len: text.len(),
                font: font(bold, italic),
                color: color(fg),
                background_color: None,
                underline: underline.then(|| UnderlineStyle { color: Some(color(fg)), thickness: px(1.), wavy: false }),
                strikethrough: strike.then(|| gpui::StrikethroughStyle { color: Some(color(fg)), thickness: px(1.) }),
            };
            window.text_system().shape_line(text.to_owned().into(), px(p.font_size), &[run], None)
        };

        for (r, segs) in p.rows.iter().enumerate() {
            for s in segs {
                let line = shape(window, &s.text, s.fg, s.bold, s.italic, s.underline, s.strikeout);
                let _ = line.paint(point(x_of(s.col), y_of(r)), lh, window, cx);
            }
        }

        if let Some((row, col, kind, cell)) = &p.cursor {
            let (x, y) = (x_of(*col), y_of(*row));
            let w = px(p.cell_w * if cell.style.wide { 2.0 } else { 1.0 });
            let cc = color(Rgb8::hex(ui().term_cursor));
            if !p.focused {
                // Unfocused: outline only.
                for r in [
                    Bounds::new(point(x, y), size(w, px(1.))),
                    Bounds::new(point(x, y + lh - px(1.)), size(w, px(1.))),
                    Bounds::new(point(x, y), size(px(1.), lh)),
                    Bounds::new(point(x + w - px(1.), y), size(px(1.), lh)),
                ] {
                    window.paint_quad(fill(r, cc));
                }
            } else {
                match kind {
                    CursorKind::Bar => window.paint_quad(fill(Bounds::new(point(x, y), size(px(2.), lh)), cc)),
                    CursorKind::Underline => {
                        window.paint_quad(fill(Bounds::new(point(x, y + lh - px(2.)), size(w, px(2.))), cc))
                    }
                    CursorKind::Block | CursorKind::Hollow => {
                        window.paint_quad(fill(Bounds::new(point(x, y), size(w, lh)), cc));
                        // Redraw the character under the block in the background color.
                        if cell.ch != ' ' && cell.ch != '\0' {
                            let line = shape(
                                window,
                                &cell.ch.to_string(),
                                p.default_bg,
                                cell.style.bold,
                                cell.style.italic,
                                false,
                                false,
                            );
                            let _ = line.paint(point(x, y), lh, window, cx);
                        }
                    }
                }
            }
        }
    }
}
