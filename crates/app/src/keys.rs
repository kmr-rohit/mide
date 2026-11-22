//! Keyboard input -> the bytes a terminal program expects (xterm conventions).

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Mods {
    fn any(self) -> bool {
        self.ctrl || self.alt || self.shift
    }
    /// xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4).
    fn param(self) -> u8 {
        1 + self.shift as u8 + 2 * self.alt as u8 + 4 * self.ctrl as u8
    }
}

/// `key` is GPUI's key name ("enter", "up", "a", ...); `key_char` is the text
/// the keystroke would type, if any. Returns `None` for keys that produce nothing.
pub fn encode(key: &str, key_char: Option<&str>, m: Mods, app_cursor: bool) -> Option<Vec<u8>> {
    let csi = |body: String| Some(format!("\x1b[{body}").into_bytes());
    let esc_prefixed = |mut b: Vec<u8>| {
        if m.alt {
            b.insert(0, 0x1b);
        }
        Some(b)
    };

    // Cursor-like keys: ESC [ A, ESC O A in application mode, ESC [ 1;m A with modifiers.
    let letter = match key {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        "home" => Some('H'),
        "end" => Some('F'),
        _ => None,
    };
    if let Some(l) = letter {
        return if m.any() {
            csi(format!("1;{}{l}", m.param()))
        } else if app_cursor {
            Some(format!("\x1bO{l}").into_bytes())
        } else {
            csi(l.to_string())
        };
    }

    let tilde = match key {
        "insert" => Some(2),
        "delete" => Some(3),
        "pageup" => Some(5),
        "pagedown" => Some(6),
        "f5" => Some(15),
        "f6" => Some(17),
        "f7" => Some(18),
        "f8" => Some(19),
        "f9" => Some(20),
        "f10" => Some(21),
        "f11" => Some(23),
        "f12" => Some(24),
        _ => None,
    };
    if let Some(n) = tilde {
        return if m.any() { csi(format!("{n};{}~", m.param())) } else { csi(format!("{n}~")) };
    }

    if let Some(p) = match key {
        "f1" => Some('P'),
        "f2" => Some('Q'),
        "f3" => Some('R'),
        "f4" => Some('S'),
        _ => None,
    } {
        return if m.any() { csi(format!("1;{}{p}", m.param())) } else { Some(format!("\x1bO{p}").into_bytes()) };
    }

    match key {
        "enter" => return esc_prefixed(vec![b'\r']),
        "tab" if m.shift => return csi("Z".into()),
        "tab" => return esc_prefixed(vec![b'\t']),
        "escape" => return Some(vec![0x1b]),
        "backspace" if m.ctrl => return Some(vec![0x08]),
        "backspace" => return esc_prefixed(vec![0x7f]),
        "space" if m.ctrl => return Some(vec![0]),
        _ => {}
    }

    let single = (key.chars().count() == 1).then(|| key.chars().next().unwrap());
    if m.ctrl {
        let c = single?;
        let byte = match c.to_ascii_lowercase() {
            l @ 'a'..='z' => l as u8 - b'a' + 1,
            '[' => 0x1b,
            '\\' => 0x1c,
            ']' => 0x1d,
            '^' | '6' => 0x1e,
            '_' | '-' | '/' => 0x1f,
            '@' | '2' | ' ' => 0,
            _ => return None,
        };
        return esc_prefixed(vec![byte]);
    }
    if m.alt {
        // Option acts as Meta: ESC + the unmodified key (shifted if shift is held).
        let c = single.or_else(|| key_char.and_then(|s| s.chars().next()))?;
        let c = if m.shift { key_char.and_then(|s| s.chars().next()).unwrap_or(c.to_ascii_uppercase()) } else { c };
        return Some(format!("\x1b{c}").into_bytes());
    }

    // Plain typing.
    let text = key_char.filter(|s| !s.is_empty()).or_else(|| single.is_some().then_some(key))?;
    (!text.chars().any(char::is_control)).then(|| text.as_bytes().to_vec())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseBtn {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    /// Motion with no button held (only reported in all-motion mode).
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Press,
    Release,
    Motion,
}

/// A mouse report for a program that asked for them (DECSET 1000/1002/1003).
/// `col`/`row` are 0-based cells. `sgr` selects the modern encoding (1006); the
/// legacy one cannot address cells past 222 and returns `None` there.
pub fn encode_mouse(btn: MouseBtn, kind: MouseKind, m: Mods, col: usize, row: usize, sgr: bool) -> Option<Vec<u8>> {
    let wheel = matches!(btn, MouseBtn::WheelUp | MouseBtn::WheelDown);
    if wheel && kind != MouseKind::Press {
        return None;
    }
    let base: u32 = match btn {
        MouseBtn::Left => 0,
        MouseBtn::Middle => 1,
        MouseBtn::Right => 2,
        MouseBtn::WheelUp => 64,
        MouseBtn::WheelDown => 65,
        MouseBtn::None => 3,
    };
    let bits = 4 * m.shift as u32 + 8 * m.alt as u32 + 16 * m.ctrl as u32 + if kind == MouseKind::Motion { 32 } else { 0 };
    let (x, y) = (col + 1, row + 1);
    if sgr {
        let end = if kind == MouseKind::Release { 'm' } else { 'M' };
        Some(format!("\x1b[<{};{x};{y}{end}", base + bits).into_bytes())
    } else {
        // Legacy: a release is "button 3"; every value is offset by 32 and must fit a byte.
        let b = if kind == MouseKind::Release { 3 } else { base };
        let (cb, cx, cy) = (32 + b + bits, 32 + x as u32, 32 + y as u32);
        (cb <= 255 && cx <= 255 && cy <= 255).then(|| vec![0x1b, b'[', b'M', cb as u8, cx as u8, cy as u8])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { ctrl: false, alt: false, shift: false };
    const CTRL: Mods = Mods { ctrl: true, alt: false, shift: false };
    const ALT: Mods = Mods { ctrl: false, alt: true, shift: false };
    const SHIFT: Mods = Mods { ctrl: false, alt: false, shift: true };

    fn enc(key: &str, ch: Option<&str>, m: Mods) -> Option<Vec<u8>> {
        encode(key, ch, m, false)
    }

    #[test]
    fn printable_text_and_unicode() {
        assert_eq!(enc("a", Some("a"), NONE), Some(b"a".to_vec()));
        assert_eq!(enc("a", Some("A"), SHIFT), Some(b"A".to_vec()));
        assert_eq!(enc("space", Some(" "), NONE), Some(b" ".to_vec()));
        assert_eq!(enc("e", Some("é"), NONE), Some("é".as_bytes().to_vec()));
    }

    #[test]
    fn control_characters() {
        assert_eq!(enc("c", Some("c"), CTRL), Some(vec![3]));
        assert_eq!(enc("d", None, CTRL), Some(vec![4]));
        assert_eq!(enc("z", None, CTRL), Some(vec![26]));
        assert_eq!(enc("[", None, CTRL), Some(vec![0x1b]));
        assert_eq!(enc("space", Some(" "), CTRL), Some(vec![0]));
        assert_eq!(enc("1", None, CTRL), None);
    }

    #[test]
    fn enter_tab_escape_backspace() {
        assert_eq!(enc("enter", None, NONE), Some(b"\r".to_vec()));
        assert_eq!(enc("tab", None, NONE), Some(b"\t".to_vec()));
        assert_eq!(enc("tab", None, SHIFT), Some(b"\x1b[Z".to_vec()));
        assert_eq!(enc("escape", None, NONE), Some(vec![0x1b]));
        assert_eq!(enc("backspace", None, NONE), Some(vec![0x7f]));
        assert_eq!(enc("backspace", None, ALT), Some(vec![0x1b, 0x7f]));
    }

    #[test]
    fn arrows_respect_application_mode_and_modifiers() {
        assert_eq!(enc("up", None, NONE), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode("up", None, NONE, true), Some(b"\x1bOA".to_vec()));
        assert_eq!(enc("left", None, ALT), Some(b"\x1b[1;3D".to_vec()));
        assert_eq!(enc("right", None, CTRL), Some(b"\x1b[1;5C".to_vec()));
        // Modifiers win over application mode.
        assert_eq!(encode("down", None, SHIFT, true), Some(b"\x1b[1;2B".to_vec()));
        assert_eq!(enc("home", None, NONE), Some(b"\x1b[H".to_vec()));
    }

    #[test]
    fn navigation_and_function_keys() {
        assert_eq!(enc("pageup", None, NONE), Some(b"\x1b[5~".to_vec()));
        assert_eq!(enc("delete", None, NONE), Some(b"\x1b[3~".to_vec()));
        assert_eq!(enc("delete", None, CTRL), Some(b"\x1b[3;5~".to_vec()));
        assert_eq!(enc("f1", None, NONE), Some(b"\x1bOP".to_vec()));
        assert_eq!(enc("f5", None, NONE), Some(b"\x1b[15~".to_vec()));
        assert_eq!(enc("f12", None, SHIFT), Some(b"\x1b[24;2~".to_vec()));
    }

    #[test]
    fn option_acts_as_meta() {
        // macOS composes option+b to "∫"; terminals want ESC b (word back).
        assert_eq!(enc("b", Some("∫"), ALT), Some(b"\x1bb".to_vec()));
        assert_eq!(enc("f", Some("ƒ"), ALT), Some(b"\x1bf".to_vec()));
        assert_eq!(enc("enter", None, ALT), Some(b"\x1b\r".to_vec()));
    }

    #[test]
    fn unmapped_keys_produce_nothing() {
        assert_eq!(enc("shift", None, SHIFT), None);
        assert_eq!(enc("capslock", None, NONE), None);
    }

    #[test]
    fn sgr_mouse_reports() {
        use MouseBtn::*;
        use MouseKind::*;
        let enc = |b, kind, m, c, r| encode_mouse(b, kind, m, c, r, true).map(|v| String::from_utf8(v).unwrap());
        assert_eq!(enc(Left, Press, NONE, 0, 0).as_deref(), Some("\x1b[<0;1;1M"));
        assert_eq!(enc(Left, Release, NONE, 9, 4).as_deref(), Some("\x1b[<0;10;5m"));
        assert_eq!(enc(Right, Press, NONE, 2, 3).as_deref(), Some("\x1b[<2;3;4M"));
        assert_eq!(enc(WheelUp, Press, NONE, 5, 5).as_deref(), Some("\x1b[<64;6;6M"));
        assert_eq!(enc(WheelDown, Press, CTRL, 0, 0).as_deref(), Some("\x1b[<81;1;1M"));
        assert_eq!(enc(Left, Motion, SHIFT, 1, 1).as_deref(), Some("\x1b[<36;2;2M")); // 0 + 4 + 32
        assert_eq!(enc(None, Motion, NONE, 1, 1).as_deref(), Some("\x1b[<35;2;2M"));
        assert_eq!(enc(WheelUp, Release, NONE, 0, 0), Option::None); // wheels have no release
        assert_eq!(enc(Left, Press, ALT, 300, 300).as_deref(), Some("\x1b[<8;301;301M")); // SGR has no limit
    }

    #[test]
    fn legacy_mouse_reports() {
        use MouseBtn::*;
        use MouseKind::*;
        assert_eq!(encode_mouse(Left, Press, NONE, 0, 0, false), Some(vec![0x1b, b'[', b'M', 32, 33, 33]));
        assert_eq!(encode_mouse(Left, Release, NONE, 0, 0, false), Some(vec![0x1b, b'[', b'M', 35, 33, 33]));
        assert_eq!(encode_mouse(WheelUp, Press, NONE, 1, 2, false), Some(vec![0x1b, b'[', b'M', 96, 34, 35]));
        assert_eq!(encode_mouse(Left, Press, NONE, 300, 0, false), Option::None); // does not fit a byte
    }
}
