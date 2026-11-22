//! Color themes: a built-in dark default plus a loader for Sublime Text
//! `.sublime-color-scheme` files.
//!
//! Scope matching follows Sublime's rules: per property (foreground,
//! font style) the most specific matching rule wins; ties go to the later rule.

use std::collections::HashMap;
use std::str::FromStr;

use syntect::highlighting::ScopeSelectors;
use syntect::parsing::Scope;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn hex(v: u32) -> Self {
        Self { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Style {
    pub fg: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

#[derive(Debug)]
pub struct Rule {
    selectors: ScopeSelectors,
    fg: Option<Rgb>,
    /// `Some` when the rule sets `font_style` (an empty string resets it).
    font: Option<FontStyle>,
}

#[derive(Debug, Clone, Copy, Default)]
struct FontStyle {
    bold: bool,
    italic: bool,
    underline: bool,
}

#[derive(Debug)]
pub struct Theme {
    pub name: String,
    pub background: Rgb,
    pub foreground: Rgb,
    pub caret: Rgb,
    pub selection: Rgb,
    pub line_highlight: Rgb,
    pub gutter_foreground: Rgb,
    rules: Vec<Rule>,
}

#[derive(Debug)]
pub enum ThemeError {
    Json(String),
    Invalid(String),
}

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThemeError::Json(e) => write!(f, "invalid color scheme JSON: {e}"),
            ThemeError::Invalid(e) => write!(f, "invalid color scheme: {e}"),
        }
    }
}
impl std::error::Error for ThemeError {}

impl Theme {
    pub fn base_style(&self) -> Style {
        Style { fg: self.foreground, bold: false, italic: false, underline: false }
    }

    /// Resolve the style for a scope stack (outermost scope first).
    pub fn style_for(&self, stack: &[Scope]) -> Style {
        let mut style = self.base_style();
        let (mut fg_power, mut font_power) = (-1.0f64, -1.0f64);
        for rule in &self.rules {
            let Some(power) = rule.selectors.does_match(stack) else { continue };
            let p = power.0;
            if let Some(fg) = rule.fg {
                if p >= fg_power {
                    fg_power = p;
                    style.fg = fg;
                }
            }
            if let Some(font) = rule.font {
                if p >= font_power {
                    font_power = p;
                    style.bold = font.bold;
                    style.italic = font.italic;
                    style.underline = font.underline;
                }
            }
        }
        style
    }

    /// Built-in dark theme matching the look in DESIGN.md.
    pub fn default_dark() -> Self {
        const RULES: &[(&str, u32, &str)] = &[
            ("comment, punctuation.definition.comment", 0x65737e, ""),
            ("string, punctuation.definition.string", 0x99c794, ""),
            ("constant.numeric", 0xf9ae58, ""),
            ("constant.language, constant.other, support.constant", 0xf99157, ""),
            ("constant.character.escape, constant.character", 0x5fb3b3, ""),
            ("keyword, storage, punctuation.definition.keyword", 0xc594c5, ""),
            ("keyword.operator, keyword.control.flow.assignment", 0xf97b58, ""),
            ("entity.name.function, support.function, variable.function", 0x6699cc, ""),
            ("entity.name.class, entity.name.type, entity.name.struct, entity.name.enum, entity.name.trait, support.class, support.type, storage.type", 0xfac863, ""),
            ("variable.parameter", 0xf9ae58, ""),
            ("variable.language", 0xec5f66, "italic"),
            ("entity.name.tag", 0xec5f66, ""),
            ("entity.other.attribute-name", 0xfac863, ""),
            ("entity.name.tag.yaml, meta.mapping.key.yaml, entity.name.label", 0x5fb3b3, ""),
            ("meta.mapping.key.json string, meta.mapping.key.json punctuation.definition.string, support.type.property-name", 0xd8dee9, ""),
            ("markup.heading, entity.name.section", 0x6699cc, "bold"),
            ("markup.bold", 0xfac863, "bold"),
            ("markup.italic", 0xc594c5, "italic"),
            ("markup.raw, markup.inline.raw", 0x99c794, ""),
            ("markup.underline.link", 0x6699cc, "underline"),
            ("markup.inserted", 0x99c794, ""),
            ("markup.deleted", 0xec5f66, ""),
            ("invalid", 0xec5f66, ""),
        ];
        let rules = RULES
            .iter()
            .map(|(sel, color, font)| Rule {
                selectors: ScopeSelectors::from_str(sel).expect("builtin selector"),
                fg: Some(Rgb::hex(*color)),
                font: Some(parse_font_style(font)),
            })
            .collect();
        Theme {
            name: "Mide Dark".into(),
            background: Rgb::hex(0x303942),
            foreground: Rgb::hex(0xd8dee9),
            caret: Rgb::hex(0xf8f8f0),
            selection: Rgb::hex(0x4e5a65),
            line_highlight: Rgb::hex(0x3a444d),
            gutter_foreground: Rgb::hex(0x7a8691),
            rules,
        }
    }

    /// Parse a Sublime Text `.sublime-color-scheme` (JSON with comments and
    /// trailing commas allowed). Supports `var(...)`; `color(...)` modifiers
    /// are reduced to their base color.
    pub fn from_sublime_color_scheme(text: &str) -> Result<Self, ThemeError> {
        let value: serde_json::Value =
            serde_json::from_str(&clean_json(text)).map_err(|e| ThemeError::Json(e.to_string()))?;
        let obj = value.as_object().ok_or_else(|| ThemeError::Invalid("top level is not an object".into()))?;

        let vars: HashMap<String, String> = obj
            .get("variables")
            .and_then(|v| v.as_object())
            .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect())
            .unwrap_or_default();
        let color = |s: &str| parse_color(s, &vars, 0);

        let globals = obj.get("globals").and_then(|g| g.as_object());
        let global = |key: &str| globals.and_then(|g| g.get(key)).and_then(|v| v.as_str()).and_then(&color);
        let background = global("background").unwrap_or(Rgb::hex(0xffffff));
        let foreground = global("foreground").unwrap_or(Rgb::hex(0x000000));

        let mut rules = Vec::new();
        for r in obj.get("rules").and_then(|r| r.as_array()).into_iter().flatten() {
            let Some(r) = r.as_object() else { continue };
            let Some(scope) = r.get("scope").and_then(|s| s.as_str()) else { continue };
            let Ok(selectors) = ScopeSelectors::from_str(scope) else { continue };
            let fg = r.get("foreground").and_then(|v| v.as_str()).and_then(&color);
            let font = r.get("font_style").and_then(|v| v.as_str()).map(parse_font_style);
            if fg.is_some() || font.is_some() {
                rules.push(Rule { selectors, fg, font });
            }
        }

        Ok(Theme {
            name: obj.get("name").and_then(|n| n.as_str()).unwrap_or("Untitled").to_owned(),
            background,
            foreground,
            caret: global("caret").unwrap_or(foreground),
            selection: global("selection").unwrap_or(background),
            line_highlight: global("line_highlight").unwrap_or(background),
            gutter_foreground: global("gutter_foreground").unwrap_or(foreground),
            rules,
        })
    }
}

fn parse_font_style(s: &str) -> FontStyle {
    let mut f = FontStyle::default();
    for word in s.split_whitespace() {
        match word {
            "bold" => f.bold = true,
            "italic" => f.italic = true,
            "underline" => f.underline = true,
            _ => {}
        }
    }
    f
}

fn parse_color(s: &str, vars: &HashMap<String, String>, depth: u8) -> Option<Rgb> {
    if depth > 8 {
        return None; // variable cycle
    }
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        let h = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        return match hex.len() {
            6 | 8 => Some(Rgb { r: h(0)?, g: h(2)?, b: h(4)? }),
            3 | 4 => {
                let d = |i: usize| u8::from_str_radix(hex.get(i..i + 1)?, 16).ok().map(|v| v * 17);
                Some(Rgb { r: d(0)?, g: d(1)?, b: d(2)? })
            }
            _ => None,
        };
    }
    if let Some(inner) = s.strip_prefix("var(").and_then(|r| r.strip_suffix(')')) {
        return parse_color(vars.get(inner.trim())?, vars, depth + 1);
    }
    if let Some(inner) = s.strip_prefix("color(").and_then(|r| r.strip_suffix(')')) {
        // `color(base modifier(...) ...)`: use the base, ignore modifiers.
        let end = if inner.starts_with("var(") { inner.find(')').map(|i| i + 1) } else { inner.find(' ') }
            .unwrap_or(inner.len());
        return parse_color(&inner[..end], vars, depth + 1);
    }
    None
}

/// Strip `//` and `/* */` comments and trailing commas, leaving strings intact.
fn clean_json(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let (mut i, mut in_str) = (0, false);
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 1;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        } else if c == ',' {
            // Drop trailing commas: next non-space char closes a container.
            let next = chars[i + 1..].iter().find(|ch| !ch.is_whitespace());
            if !matches!(next, Some('}') | Some(']')) {
                out.push(c);
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}
