use super::*;

fn setup(name: &str) -> (Arc<Syntaxes>, Arc<Theme>, String) {
    let s = Syntaxes::load_default();
    let n = s.names().into_iter().find(|n| *n == name).unwrap_or_else(|| panic!("no syntax {name}; have {:?}", s.names())).to_owned();
    (s, Arc::new(Theme::default_dark()), n)
}

fn color_at(spans: &[Span], byte: usize) -> Rgb {
    spans.iter().find(|s| s.start <= byte && byte < s.end).unwrap().style.fg
}

#[test]
fn detects_syntax_by_extension_name_and_shebang() {
    let s = Syntaxes::load_default();
    assert_eq!(s.detect(Some(Path::new("a/b/deploy.yaml")), ""), "YAML");
    assert_eq!(s.detect(Some(Path::new("main.rs")), ""), "Rust");
    assert_eq!(s.detect(Some(Path::new("Cargo.toml")), ""), "TOML");
    assert_eq!(s.detect(Some(Path::new("Dockerfile")), ""), "Dockerfile");
    assert_eq!(s.detect(None, "#!/usr/bin/env python3"), "Python");
    assert_eq!(s.detect(Some(Path::new("notes.unknownext")), "just text"), "Plain Text");
}

#[test]
fn yaml_keys_strings_and_numbers_get_theme_colors() {
    let (s, t, n) = setup("YAML");
    let mut h = DocHighlighter::new(s, t, &n);
    let lines = ["kind: ConfigMap", "name: \"demo-api\"", "replicas: 3"];
    let out = h.highlight_range(0, 3, |i| lines[i].to_owned());

    assert_eq!(color_at(&out[0], 0), Rgb::hex(0x5fb3b3)); // key: teal, like the screenshot
    assert_eq!(color_at(&out[1], 7), Rgb::hex(0x99c794)); // string
    assert_eq!(color_at(&out[2], 10), Rgb::hex(0xf9ae58)); // number
    // Spans tile the whole line.
    for (spans, line) in out.iter().zip(lines) {
        assert_eq!(spans.first().unwrap().start, 0);
        assert_eq!(spans.last().unwrap().end, line.len());
        assert!(spans.windows(2).all(|w| w[0].end == w[1].start));
    }
}

#[test]
fn multiline_state_carries_across_lines() {
    let (s, t, n) = setup("Rust");
    let mut h = DocHighlighter::new(s, t, &n);
    let lines = ["/* start", "still comment", "end */ let x = 1;"];
    let out = h.highlight_range(0, 3, |i| lines[i].to_owned());
    let comment = Rgb::hex(0x65737e);
    assert_eq!(color_at(&out[1], 3), comment);
    assert_eq!(color_at(&out[2], 0), comment);
    assert_ne!(color_at(&out[2], 10), comment); // `let` is a keyword again
}

fn big_doc() -> Vec<String> {
    (0..300)
        .map(|i| match i % 5 {
            0 => format!("fn f{i}() {{"),
            1 => format!("    let s = \"line {i}\"; // note"),
            2 => "    /* open".to_owned(),
            3 => "    close */".to_owned(),
            _ => "}".to_owned(),
        })
        .collect()
}

#[test]
fn incremental_matches_fresh_after_edit() {
    let (s, t, n) = setup("Rust");
    let mut doc = big_doc();
    let mut inc = DocHighlighter::new(s.clone(), t.clone(), &n);
    let all = |h: &mut DocHighlighter, doc: &Vec<String>| h.highlight_range(0, doc.len(), |i| doc[i].clone());
    let _ = all(&mut inc, &doc);
    assert!(inc.checkpoints.len() > 1);

    // Open a block comment on line 70 that swallows everything after it.
    doc[70] = "/* oops".to_owned();
    inc.invalidate_from(70);
    let a = all(&mut inc, &doc);
    let mut fresh = DocHighlighter::new(s, t, &n);
    let b = all(&mut fresh, &doc);
    assert_eq!(a, b);
}

#[test]
fn windowed_range_matches_full_pass() {
    let (s, t, n) = setup("Rust");
    let doc = big_doc();
    let mut h1 = DocHighlighter::new(s.clone(), t.clone(), &n);
    let full = h1.highlight_range(0, doc.len(), |i| doc[i].clone());
    // Ask for a window deep in the file first (cold checkpoints), then another.
    let mut h2 = DocHighlighter::new(s, t, &n);
    let w = h2.highlight_range(200, 20, |i| doc[i].clone());
    assert_eq!(w, full[200..220]);
    let w = h2.highlight_range(130, 10, |i| doc[i].clone());
    assert_eq!(w, full[130..140]);
}

#[test]
fn very_long_lines_are_not_parsed() {
    let (s, t, n) = setup("JSON");
    let mut h = DocHighlighter::new(s, t.clone(), &n);
    let long = format!("[{}]", "1,".repeat(10_000));
    let out = h.highlight_range(0, 1, |_| long.clone());
    assert_eq!(out[0].len(), 1);
    assert_eq!(out[0][0].style, t.base_style());
}

#[test]
fn sublime_color_scheme_loader() {
    let scheme = r##"
    // a comment
    {
        "name": "Test",
        "variables": { "teal": "#5fb3b3", "alias": "var(teal)", },
        "globals": { "background": "#112233", "foreground": "#eeeeee", "caret": "var(teal)" },
        "rules": [
            { "scope": "comment", "foreground": "#808080", "font_style": "italic" },
            { "scope": "string", "foreground": "color(var(alias) alpha(0.5))" },
            { "scope": "string.quoted.double", "font_style": "bold" /* trailing */ },
        ]
    }"##;
    let t = Theme::from_sublime_color_scheme(scheme).unwrap();
    assert_eq!(t.name, "Test");
    assert_eq!(t.background, Rgb::hex(0x112233));
    assert_eq!(t.caret, Rgb::hex(0x5fb3b3));

    let scope = |s: &str| vec![Scope::new(s).unwrap()];
    let c = t.style_for(&scope("comment.line"));
    assert!(c.italic && !c.bold);
    assert_eq!(c.fg, Rgb::hex(0x808080));
    // Foreground from the generic rule, font style from the more specific one.
    let s = t.style_for(&scope("string.quoted.double.json"));
    assert_eq!(s.fg, Rgb::hex(0x5fb3b3));
    assert!(s.bold);
    // Unmatched scopes get the global foreground.
    assert_eq!(t.style_for(&scope("source.rust")).fg, Rgb::hex(0xeeeeee));
}

#[test]
fn bad_scheme_reports_error() {
    assert!(Theme::from_sublime_color_scheme("not json").is_err());
    assert!(Theme::from_sublime_color_scheme("[1,2]").is_err());
}

fn many_lines(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| match i % 4 {
            0 => format!("fn f{i}() {{"),
            1 => format!("    let s = \"line {i}\"; // note"),
            2 => format!("    let n = {i};"),
            _ => "}".to_owned(),
        })
        .collect()
}

#[test]
fn a_far_jump_is_instant_and_matches_the_exact_result_for_ordinary_code() {
    let (s, t, n) = setup("Rust");
    let doc = many_lines(8000);

    let started = std::time::Instant::now();
    let mut h = DocHighlighter::new(s.clone(), t.clone(), &n);
    let far = h.highlight_window(7000, 30, |i| doc[i].clone());
    assert!(far.approximate, "7000 lines past anything parsed must not be parsed in full");
    assert!(started.elapsed() < std::time::Duration::from_millis(500), "took {:?}", started.elapsed());
    assert_eq!(far.lines.len(), 30);

    // Plain code has no multi-line state, so the shortcut agrees with the full parse.
    let mut exact = DocHighlighter::new(s, t, &n);
    let full = exact.highlight_range(0, 7030, |i| doc[i].clone());
    assert_eq!(far.lines, full[7000..7030]);
}

#[test]
fn nearby_windows_stay_exact() {
    let (s, t, n) = setup("Rust");
    let doc = many_lines(3000);
    let mut h = DocHighlighter::new(s, t, &n);
    assert!(!h.highlight_window(0, 40, |i| doc[i].clone()).approximate);
    assert!(!h.highlight_window(300, 40, |i| doc[i].clone()).approximate);
    assert!(!h.highlight_window(1200, 40, |i| doc[i].clone()).approximate, "well within the catch-up distance");
}
