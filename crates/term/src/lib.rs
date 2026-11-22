//! Terminal backend: spawns a shell on a PTY, runs it through a terminal
//! emulator, and hands the UI a plain [`Snapshot`] of cells with colors already
//! resolved. No UI code here.

use std::borrow::Cow;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg, State};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};

// ---- public types -------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
    /// Pixel size of one cell; only used for apps that query it (sixel, etc.).
    pub cell_width: u16,
    pub cell_height: u16,
}

impl Size {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self { cols, rows, cell_width: 8, cell_height: 16 }
    }
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }
    fn screen_lines(&self) -> usize {
        self.rows as usize
    }
    fn columns(&self) -> usize {
        self.cols as usize
    }
}

#[derive(Debug, Clone, Default)]
pub struct SpawnOptions {
    /// Program and args; `None` runs the user's login shell.
    pub program: Option<(String, Vec<String>)>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    /// Lines of scrollback to keep (default 10 000).
    pub scrollback: Option<usize>,
}

/// Which mouse events the running program asked to receive.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MouseMode {
    /// Button presses and releases (DECSET 1000).
    pub click: bool,
    /// Motion while a button is held (1002).
    pub drag: bool,
    /// All motion (1003).
    pub motion: bool,
    /// SGR encoding (1006) instead of the legacy byte encoding.
    pub sgr: bool,
}

impl MouseMode {
    /// The program wants mouse events, so the terminal must not use them itself.
    pub fn active(&self) -> bool {
        self.click || self.drag || self.motion
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectKind {
    Simple,
    Word,
    Line,
}

/// Things the UI should react to. Call [`Terminal::poll_events`] each frame
/// (or whenever woken).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermEvent {
    /// New output; repaint.
    Wakeup,
    Title(String),
    Bell,
    /// Text the program asked to put on the clipboard (OSC 52).
    ClipboardStore(String),
    /// Child exited with this code (`None` if killed by a signal).
    Exited(Option<i32>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb8 {
    pub const fn hex(v: u32) -> Self {
        Self { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    /// Second half of a double-width char; skip when drawing.
    pub wide_spacer: bool,
    /// First half of a double-width char; occupies two columns.
    pub wide: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapCell {
    pub ch: char,
    /// Already swapped for inverse video, dimmed, etc.
    pub fg: Rgb8,
    pub bg: Rgb8,
    pub style: CellStyle,
    /// Inside the current text selection.
    pub selected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorKind {
    Block,
    Underline,
    Bar,
    Hollow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapCursor {
    pub row: usize,
    pub col: usize,
    pub kind: CursorKind,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub cols: usize,
    pub rows: usize,
    /// Row-major, `rows * cols` cells.
    pub cells: Vec<SnapCell>,
    /// `None` when hidden or scrolled back.
    pub cursor: Option<SnapCursor>,
    /// Lines scrolled up from the bottom (0 = live).
    pub display_offset: usize,
    pub default_bg: Rgb8,
    pub default_fg: Rgb8,
}

impl Snapshot {
    pub fn cell(&self, row: usize, col: usize) -> &SnapCell {
        &self.cells[row * self.cols + col]
    }

    pub fn row_text(&self, row: usize) -> String {
        let s: String = (0..self.cols)
            .filter(|&c| !self.cell(row, c).style.wide_spacer)
            .map(|c| self.cell(row, c).ch)
            .collect();
        s.trim_end().to_owned()
    }

    pub fn text(&self) -> String {
        (0..self.rows).map(|r| self.row_text(r)).collect::<Vec<_>>().join("\n")
    }
}

/// Colors used to resolve the emulator's named/indexed colors.
#[derive(Debug, Clone)]
pub struct Palette {
    pub fg: Rgb8,
    pub bg: Rgb8,
    pub ansi: [Rgb8; 16],
}

impl Default for Palette {
    /// Matches the editor's dark scheme (see DESIGN.md).
    fn default() -> Self {
        Self {
            fg: Rgb8::hex(0xd8dee9),
            bg: Rgb8::hex(0x303942),
            ansi: [
                Rgb8::hex(0x303942), Rgb8::hex(0xec5f66), Rgb8::hex(0x99c794), Rgb8::hex(0xfac863),
                Rgb8::hex(0x6699cc), Rgb8::hex(0xc594c5), Rgb8::hex(0x5fb3b3), Rgb8::hex(0xd8dee9),
                Rgb8::hex(0x65737e), Rgb8::hex(0xf97b58), Rgb8::hex(0xaddb67), Rgb8::hex(0xffd479),
                Rgb8::hex(0x82aaff), Rgb8::hex(0xd1a3f7), Rgb8::hex(0x7fdbca), Rgb8::hex(0xffffff),
            ],
        }
    }
}

// ---- terminal ------------------------------------------------------------------

#[derive(Clone)]
struct Listener {
    tx: Sender<Event>,
    /// Called (from the I/O thread) after every event is queued, so the UI can
    /// sleep until there is something to do instead of polling.
    waker: Arc<dyn Fn() + Send + Sync>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.tx.send(event);
        (self.waker)();
    }
}

pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
    events: Receiver<Event>,
    io: Option<JoinHandle<(EventLoop<tty::Pty, Listener>, State)>>,
    size: Size,
    title: String,
    palette: Palette,
    child_pid: u32,
}

impl Terminal {
    pub fn spawn(opts: SpawnOptions, size: Size) -> io::Result<Self> {
        Self::spawn_with_waker(opts, size, || {})
    }

    /// Like [`spawn`](Self::spawn), but `waker` is invoked whenever new events are
    /// queued for [`poll_events`](Self::poll_events). It runs on another thread
    /// and must be cheap and non-blocking.
    pub fn spawn_with_waker(
        opts: SpawnOptions,
        size: Size,
        waker: impl Fn() + Send + Sync + 'static,
    ) -> io::Result<Self> {
        tty::setup_env();
        let (tx, events) = mpsc::channel();
        let listener = Listener { tx, waker: Arc::new(waker) };

        let mut config = Config::default();
        if let Some(n) = opts.scrollback {
            config.scrolling_history = n;
        }
        let term = Term::new(config, &size, listener.clone());
        let term = Arc::new(FairMutex::new(term));

        // `..Default::default()` covers the Windows-only `escape_args` field.
        #[allow(clippy::needless_update)]
        let pty_opts = tty::Options {
            shell: opts.program.map(|(p, a)| tty::Shell::new(p, a)),
            working_directory: opts.cwd,
            drain_on_exit: true,
            env: opts.env.into_iter().collect(),
            ..Default::default()
        };
        let pty = tty::new(&pty_opts, window_size(size), 0)?;
        let child_pid = pty.child().id();
        let event_loop = EventLoop::new(term.clone(), listener, pty, pty_opts.drain_on_exit, false)?;
        let sender = event_loop.channel();
        let io = event_loop.spawn();

        Ok(Self { term, sender, events, io: Some(io), size, title: String::new(), palette: Palette::default(), child_pid })
    }

    /// Process id of the program running in the terminal.
    pub fn pid(&self) -> u32 {
        self.child_pid
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn size(&self) -> Size {
        self.size
    }

    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    /// Send keyboard/paste bytes to the program.
    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        let _ = self.sender.send(Msg::Input(Cow::Owned(bytes.into())));
    }

    /// Paste text, honoring bracketed-paste mode.
    pub fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        if bracketed {
            self.write([b"\x1b[200~".as_slice(), text.replace('\x1b', "").as_bytes(), b"\x1b[201~"].concat());
        } else {
            self.write(text.replace("\r\n", "\r").replace('\n', "\r").into_bytes());
        }
    }

    pub fn resize(&mut self, size: Size) {
        if size == self.size {
            return;
        }
        self.size = size;
        self.term.lock().resize(size);
        let _ = self.sender.send(Msg::Resize(window_size(size)));
    }

    /// Scroll the viewport (positive = up into history).
    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }

    pub fn scroll_to_bottom(&self) {
        self.term.lock().scroll_display(Scroll::Bottom);
    }

    /// Drain pending events, answering terminal queries along the way.
    pub fn poll_events(&mut self) -> Vec<TermEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                Event::Wakeup => {
                    if !matches!(out.last(), Some(TermEvent::Wakeup)) {
                        out.push(TermEvent::Wakeup);
                    }
                }
                Event::Title(t) => {
                    self.title = t.clone();
                    out.push(TermEvent::Title(t));
                }
                Event::ResetTitle => {
                    self.title.clear();
                    out.push(TermEvent::Title(String::new()));
                }
                Event::Bell => out.push(TermEvent::Bell),
                Event::ClipboardStore(_, text) => out.push(TermEvent::ClipboardStore(text)),
                Event::PtyWrite(text) => self.write(text.into_bytes()),
                Event::TextAreaSizeRequest(f) => self.write(f(window_size(self.size)).into_bytes()),
                Event::ChildExit(status) => out.push(TermEvent::Exited(status.code())),
                _ => {}
            }
        }
        out
    }

    /// Selected text, if any (selection API to be added with the UI).
    pub fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string()
    }

    /// Application-cursor mode: arrows send `ESC O A` instead of `ESC [ A`.
    pub fn app_cursor(&self) -> bool {
        self.term.lock().mode().contains(TermMode::APP_CURSOR)
    }

    pub fn mouse_mode(&self) -> MouseMode {
        let mode = *self.term.lock().mode();
        MouseMode {
            click: mode.contains(TermMode::MOUSE_REPORT_CLICK),
            drag: mode.contains(TermMode::MOUSE_DRAG),
            motion: mode.contains(TermMode::MOUSE_MOTION),
            sgr: mode.contains(TermMode::SGR_MOUSE),
        }
    }

    /// On the alternate screen, the wheel should become arrow keys (DECSET 1007).
    pub fn alternate_scroll(&self) -> bool {
        self.term.lock().mode().contains(TermMode::ALTERNATE_SCROLL)
    }

    /// Lines the viewport is scrolled up into history (0 = at the bottom).
    pub fn display_offset(&self) -> usize {
        self.term.lock().grid().display_offset()
    }

    pub fn scroll_to_top(&self) {
        self.term.lock().scroll_display(Scroll::Top);
    }

    /// Scroll a page up (`true`) or down into/out of history.
    pub fn scroll_page(&self, up: bool) {
        self.term.lock().scroll_display(if up { Scroll::PageUp } else { Scroll::PageDown });
    }

    /// Full-screen program (vim, htop, less) is running.
    pub fn alt_screen(&self) -> bool {
        self.term.lock().mode().contains(TermMode::ALT_SCREEN)
    }

    /// Start a selection at a viewport cell (`row`/`col` count from the top-left).
    pub fn select(&self, kind: SelectKind, row: usize, col: usize) {
        let mut term = self.term.lock();
        let offset = term.grid().display_offset() as i32;
        let point = Point::new(Line(row as i32 - offset), Column(col));
        let ty = match kind {
            SelectKind::Simple => SelectionType::Simple,
            SelectKind::Word => SelectionType::Semantic,
            SelectKind::Line => SelectionType::Lines,
        };
        term.selection = Some(Selection::new(ty, point, Side::Left));
    }

    /// Drag the selection to a viewport cell.
    pub fn extend_selection(&self, row: usize, col: usize) {
        let mut term = self.term.lock();
        let offset = term.grid().display_offset() as i32;
        let point = Point::new(Line(row as i32 - offset), Column(col));
        if let Some(sel) = term.selection.as_mut() {
            sel.update(point, Side::Right);
        }
    }

    pub fn clear_selection(&self) {
        self.term.lock().selection = None;
    }

    pub fn snapshot(&self) -> Snapshot {
        let term = self.term.lock();
        let grid = term.grid();
        let (cols, rows) = (grid.columns(), grid.screen_lines());
        let offset = grid.display_offset();
        let colors = term.colors();
        let p = &self.palette;

        let selection = term.renderable_content().selection;
        let mut cells = Vec::with_capacity(cols * rows);
        for row in 0..rows {
            for col in 0..cols {
                let c = &grid[Point::new(Line(row as i32 - offset as i32), Column(col))];
                let mut fg = resolve(c.fg, p, colors, c.flags.contains(Flags::BOLD));
                let mut bg = resolve(c.bg, p, colors, false);
                if c.flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if c.flags.contains(Flags::DIM) {
                    fg = mix(fg, bg, 0.35);
                }
                if c.flags.contains(Flags::HIDDEN) {
                    fg = bg;
                }
                let point = Point::new(Line(row as i32 - offset as i32), Column(col));
                cells.push(SnapCell {
                    ch: c.c,
                    fg,
                    bg,
                    selected: selection.is_some_and(|r| r.contains(point)),
                    style: CellStyle {
                        bold: c.flags.contains(Flags::BOLD),
                        italic: c.flags.contains(Flags::ITALIC),
                        underline: c.flags.intersects(Flags::ALL_UNDERLINES),
                        strikeout: c.flags.contains(Flags::STRIKEOUT),
                        wide: c.flags.contains(Flags::WIDE_CHAR),
                        wide_spacer: c
                            .flags
                            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
                    },
                });
            }
        }

        let cur = grid.cursor.point;
        let cursor_row = cur.line.0 + offset as i32;
        let cursor = (term.mode().contains(TermMode::SHOW_CURSOR)
            && cursor_row >= 0
            && (cursor_row as usize) < rows)
            .then(|| SnapCursor {
                row: cursor_row as usize,
                col: cur.column.0.min(cols.saturating_sub(1)),
                kind: match term.cursor_style().shape {
                    CursorShape::Block => CursorKind::Block,
                    CursorShape::Underline => CursorKind::Underline,
                    CursorShape::Beam => CursorKind::Bar,
                    CursorShape::HollowBlock => CursorKind::Hollow,
                    CursorShape::Hidden => CursorKind::Block,
                },
            })
            .filter(|_| term.cursor_style().shape != CursorShape::Hidden);

        Snapshot { cols, rows, cells, cursor, display_offset: offset, default_bg: p.bg, default_fg: p.fg }
    }
}

impl Drop for Terminal {
    /// Ends the session without ever blocking the caller. The PTY's own `Drop`
    /// sends SIGHUP and then *waits* for the program to exit, and a program that
    /// ignores SIGHUP (a running `claude`, an editor, a stuck job) would freeze
    /// the UI thread forever, so that wait happens on a helper thread, with a
    /// SIGKILL if the program hasn't gone after a short grace period.
    fn drop(&mut self) {
        let _ = self.sender.send(Msg::Shutdown);
        let Some(io) = self.io.take() else { return };
        let pid = self.child_pid;
        std::thread::spawn(move || {
            let (done_tx, done_rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = io.join(); // the PTY (and its blocking wait) is dropped here
                let _ = done_tx.send(());
            });
            if done_rx.recv_timeout(Duration::from_millis(1500)).is_err() {
                force_kill(pid);
            }
        });
    }
}

/// SIGKILL the program and everything in its process group.
fn force_kill(pid: u32) {
    let group = format!("-{pid}");
    for target in [group.as_str(), &pid.to_string()] {
        let _ = std::process::Command::new("kill")
            .args(["-9", "--", target])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

fn window_size(s: Size) -> WindowSize {
    WindowSize { num_lines: s.rows, num_cols: s.cols, cell_width: s.cell_width, cell_height: s.cell_height }
}

// ---- color resolution --------------------------------------------------------

fn to8(c: Rgb) -> Rgb8 {
    Rgb8 { r: c.r, g: c.g, b: c.b }
}

fn mix(a: Rgb8, b: Rgb8, t: f32) -> Rgb8 {
    let m = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t).round() as u8;
    Rgb8 { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b) }
}

fn indexed(i: u8, p: &Palette) -> Rgb8 {
    match i {
        0..=15 => p.ansi[i as usize],
        16..=231 => {
            let n = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Rgb8 { r: level(n / 36), g: level((n / 6) % 6), b: level(n % 6) }
        }
        232..=255 => {
            let v = 8 + (i - 232) * 10;
            Rgb8 { r: v, g: v, b: v }
        }
    }
}

fn resolve(
    color: Color,
    p: &Palette,
    overrides: &alacritty_terminal::term::color::Colors,
    bold: bool,
) -> Rgb8 {
    match color {
        Color::Spec(rgb) => to8(rgb),
        Color::Indexed(i) => {
            if let Some(rgb) = overrides[i as usize] {
                return to8(rgb);
            }
            // Bold brightens the base 8 colors, like most terminals.
            indexed(if bold && i < 8 { i + 8 } else { i }, p)
        }
        Color::Named(n) => {
            if let Some(rgb) = overrides[n] {
                return to8(rgb);
            }
            let id = n as usize;
            match n {
                NamedColor::Foreground | NamedColor::Cursor | NamedColor::BrightForeground => p.fg,
                NamedColor::Background => p.bg,
                NamedColor::DimForeground => mix(p.fg, p.bg, 0.35),
                _ if id < 8 => p.ansi[if bold { id + 8 } else { id }],
                _ if id < 16 => p.ansi[id],
                // DimBlack..DimWhite
                _ => mix(p.ansi[(id - NamedColor::DimBlack as usize).min(7)], p.bg, 0.35),
            }
        }
    }
}

#[cfg(test)]
mod tests;
