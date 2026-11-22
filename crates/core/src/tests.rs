use super::*;

fn heads(b: &Buffer) -> Vec<usize> {
    b.selections().iter().map(|s| s.head).collect()
}

#[test]
fn type_and_undo() {
    let mut b = Buffer::default();
    b.insert("hello");
    b.insert(" world");
    assert_eq!(b.text(), "hello world");
    assert!(b.undo());
    assert_eq!(b.text(), "hello");
    assert!(b.redo());
    assert_eq!(b.text(), "hello world");
}

#[test]
fn multi_cursor_insert() {
    let mut b = Buffer::from_text("a\nb\nc");
    b.set_selections(vec![Selection::cursor(1), Selection::cursor(3), Selection::cursor(5)]);
    b.insert("!");
    assert_eq!(b.text(), "a!\nb!\nc!");
    assert_eq!(heads(&b), vec![2, 5, 8]);
    b.undo();
    assert_eq!(b.text(), "a\nb\nc");
    assert_eq!(b.selections().len(), 3);
}

#[test]
fn backspace_is_grapheme_aware() {
    let mut b = Buffer::from_text("e\u{301}x"); // é (e + combining acute) then x
    b.set_cursor(2);
    b.delete_backward();
    assert_eq!(b.text(), "x");
}

#[test]
fn replace_selection_and_overlap_merge() {
    let mut b = Buffer::from_text("abcdef");
    b.set_selections(vec![Selection::new(0, 3), Selection::new(2, 4)]);
    assert_eq!(b.selections().len(), 1);
    b.insert("X");
    assert_eq!(b.text(), "Xef");
}

#[test]
fn delete_forward_at_end_is_noop() {
    let mut b = Buffer::from_text("ab");
    b.set_cursor(2);
    b.delete_forward();
    assert_eq!(b.text(), "ab");
    assert!(!b.undo());
}

// ---- lines -----------------------------------------------------------------

#[test]
fn line_accessors() {
    let b = Buffer::from_text("one\ntwo\n\nlast");
    assert_eq!(b.len_lines(), 4);
    assert_eq!((b.line(0).as_str(), b.line_len(1)), ("one", 3));
    assert_eq!(b.line(2), "");
    assert_eq!(b.line(3), "last");
    assert_eq!(b.line(99), "");
    assert_eq!(b.line_col(5), (1, 1));
    assert_eq!(b.offset_of(1, 99), 7); // clamped to end of "two"
    // Trailing newline: ropey reports an extra empty line, which is what an editor shows.
    assert_eq!(Buffer::from_text("a\n").len_lines(), 2);
    assert_eq!(Buffer::from_text("").len_lines(), 1);
}

// ---- motion ----------------------------------------------------------------

#[test]
fn vertical_motion_remembers_goal_column() {
    let mut b = Buffer::from_text("abcdefgh\nxy\nabcdefgh");
    b.set_cursor(6); // line 0 col 6
    b.move_selections(Motion::Down(1), false);
    assert_eq!(b.line_col(heads(&b)[0]), (1, 2)); // clamped to "xy"
    b.move_selections(Motion::Down(1), false);
    assert_eq!(b.line_col(heads(&b)[0]), (2, 6)); // goal column restored
    b.move_selections(Motion::Down(1), false);
    assert_eq!(heads(&b)[0], b.len_chars()); // last line: jump to end
    b.move_selections(Motion::Up(5), false);
    assert_eq!(b.line_col(heads(&b)[0]).0, 0);
    b.move_selections(Motion::Up(1), false);
    assert_eq!(heads(&b)[0], 0); // first line: jump to start
}

#[test]
fn horizontal_motion_collapses_selection_and_extends() {
    let mut b = Buffer::from_text("hello world");
    b.set_selections(vec![Selection::new(2, 7)]);
    b.move_selections(Motion::Left, false);
    assert_eq!((b.selections()[0].anchor, b.selections()[0].head), (2, 2));
    b.set_selections(vec![Selection::new(2, 7)]);
    b.move_selections(Motion::Right, false);
    assert_eq!(heads(&b), vec![7]);

    b.set_cursor(3);
    b.move_selections(Motion::Right, true);
    b.move_selections(Motion::Right, true);
    let s = b.selections()[0];
    assert_eq!((s.anchor, s.head), (3, 5));
    assert_eq!(b.selected_text(), "lo");
}

#[test]
fn word_motion() {
    let mut b = Buffer::from_text("foo_bar  baz.qux");
    b.set_cursor(0);
    let mut stops = Vec::new();
    for _ in 0..4 {
        b.move_selections(Motion::WordRight, false);
        stops.push(heads(&b)[0]);
    }
    assert_eq!(stops, vec![7, 12, 13, 16]); // foo_bar | baz | . | qux
    let mut back = Vec::new();
    for _ in 0..4 {
        b.move_selections(Motion::WordLeft, false);
        back.push(heads(&b)[0]);
    }
    assert_eq!(back, vec![13, 12, 9, 0]);
}

#[test]
fn smart_home_toggles_between_indent_and_column_zero() {
    let mut b = Buffer::from_text("    code");
    b.set_cursor(8);
    b.move_selections(Motion::LineStart, false);
    assert_eq!(heads(&b), vec![4]);
    b.move_selections(Motion::LineStart, false);
    assert_eq!(heads(&b), vec![0]);
    b.move_selections(Motion::LineStart, false);
    assert_eq!(heads(&b), vec![4]);
    b.move_selections(Motion::LineEnd, false);
    assert_eq!(heads(&b), vec![8]);
}

#[test]
fn select_word_and_line() {
    let mut b = Buffer::from_text("let foo_bar = 1;\nnext");
    b.select_word_at(6);
    assert_eq!(b.selected_text(), "foo_bar");
    b.select_line_at(3);
    assert_eq!(b.selected_text(), "let foo_bar = 1;\n");
    b.select_all();
    assert_eq!(b.selected_text(), b.text());
}

// ---- editing ---------------------------------------------------------------

#[test]
fn newline_keeps_indent_but_not_past_cursor() {
    let mut b = Buffer::from_text("    foo");
    b.set_cursor(7);
    b.insert_newline();
    assert_eq!(b.text(), "    foo\n    ");
    // Cursor inside the indentation only copies what's before it.
    let mut b = Buffer::from_text("    foo");
    b.set_cursor(2);
    b.insert_newline();
    assert_eq!(b.text(), "  \n    foo");
}

#[test]
fn tab_inserts_to_next_stop() {
    let mut b = Buffer::from_text("ab");
    b.set_cursor(2);
    b.insert_tab(4);
    assert_eq!(b.text(), "ab  ");
    b.insert_tab(4);
    assert_eq!(b.text(), "ab      ");
}

#[test]
fn delete_word_backward_and_cut() {
    let mut b = Buffer::from_text("foo bar");
    b.set_cursor(7);
    b.delete_word_backward();
    assert_eq!(b.text(), "foo ");
    b.set_selections(vec![Selection::new(0, 3)]);
    b.delete_selection();
    assert_eq!(b.text(), " ");
}

#[test]
fn paste_normalizes_line_endings() {
    let mut b = Buffer::default();
    b.insert("a\r\nb\rc");
    assert_eq!(b.text(), "a\nb\nc");
}

#[test]
fn last_edit_line_tracks_edit_undo_redo() {
    let mut b = Buffer::from_text("a\nb\nc\nd");
    b.set_cursor(b.offset_of(2, 0));
    b.insert("X");
    assert_eq!(b.last_edit_line(), 2);
    b.set_cursor(0);
    b.insert("Y");
    assert_eq!(b.last_edit_line(), 0);
    b.undo();
    b.undo();
    assert_eq!(b.last_edit_line(), 2);
}

// ---- files & dirty tracking --------------------------------------------------

#[test]
fn dirty_tracks_edits_undo_and_save() {
    let dir = std::env::temp_dir().join(format!("mide-core-{}-dirty", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("f.txt");
    std::fs::write(&path, "hi\n").unwrap();

    let mut b = Buffer::open(&path).unwrap();
    assert!(!b.is_dirty());
    b.set_cursor(2);
    b.insert("!");
    assert!(b.is_dirty());
    b.undo();
    assert!(!b.is_dirty(), "undoing back to the saved state is clean");
    b.redo();
    assert!(b.is_dirty());
    b.save(&path).unwrap();
    assert!(!b.is_dirty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hi!\n");
    b.insert("?");
    assert!(b.is_dirty());
    b.undo();
    assert!(!b.is_dirty());
    assert!(!dir.join(".f.txt.mide-tmp").exists(), "temp file cleaned up");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn crlf_round_trips() {
    let dir = std::env::temp_dir().join(format!("mide-core-{}-crlf", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("w.txt");
    std::fs::write(&path, "one\r\ntwo\r\n").unwrap();
    let mut b = Buffer::open(&path).unwrap();
    assert_eq!(b.line_ending(), LineEnding::Crlf);
    assert_eq!(b.text(), "one\ntwo\n");
    b.set_cursor(3);
    b.insert("!");
    b.save(&path).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"one!\r\ntwo\r\n");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn bom_is_stripped_and_invalid_utf8_is_never_overwritten() {
    let dir = std::env::temp_dir().join(format!("mide-core-{}-enc", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bom = dir.join("bom.txt");
    std::fs::write(&bom, b"\xef\xbb\xbfhello").unwrap();
    assert_eq!(Buffer::open(&bom).unwrap().text(), "hello");

    let bad = dir.join("bad.bin");
    std::fs::write(&bad, b"ok \xff\xfe bytes").unwrap();
    let mut b = Buffer::open(&bad).unwrap();
    assert!(b.is_lossy());
    b.insert("x");
    assert!(b.save(&bad).is_err());
    assert_eq!(std::fs::read(&bad).unwrap(), b"ok \xff\xfe bytes", "original left untouched");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn with_line_matches_line_for_every_line() {
    let b = Buffer::from_text("one\r\ntwo\n\nthree");
    for i in 0..b.len_lines() + 2 {
        assert_eq!(b.with_line(i, str::to_owned), b.line(i), "line {i}");
    }
}
