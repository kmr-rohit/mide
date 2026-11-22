use super::*;
use std::time::Duration;

fn buf(text: &str) -> Buffer {
    Buffer::from_text(text)
}

fn at(b: &mut Buffer, line: usize, col: usize) {
    let o = b.offset_of(line, col);
    b.set_cursor(o);
}

fn select(b: &mut Buffer, from: (usize, usize), to: (usize, usize)) {
    let (a, h) = (b.offset_of(from.0, from.1), b.offset_of(to.0, to.1));
    b.set_selections(vec![Selection::new(a, h)]);
}

fn heads(b: &Buffer) -> Vec<(usize, usize)> {
    b.selections().iter().map(|s| b.line_col(s.head)).collect()
}

#[test]
fn typing_groups_into_one_undo_step_and_enter_breaks_it() {
    let mut b = buf("");
    for c in "hello".chars() {
        b.insert(&c.to_string());
    }
    assert_eq!(b.text(), "hello");
    b.undo();
    assert_eq!(b.text(), "", "five keystrokes undo as one step");
    b.redo();
    assert_eq!(b.text(), "hello");

    b.insert_newline();
    for c in "ab".chars() {
        b.insert(&c.to_string());
    }
    b.undo();
    assert_eq!(b.text(), "hello\n", "Enter starts a new step; typing after it is its own");
    b.undo();
    assert_eq!(b.text(), "hello");
}

#[test]
fn a_pause_or_a_moved_caret_starts_a_new_undo_step() {
    let mut b = buf("");
    b.set_undo_grouping(Duration::ZERO);
    b.insert("a");
    b.insert("b");
    b.undo();
    assert_eq!(b.text(), "a", "with a zero window nothing groups");

    let mut b = buf("xyz");
    at(&mut b, 0, 3);
    b.insert("1");
    at(&mut b, 0, 0);
    b.insert("2");
    b.undo();
    assert_eq!(b.text(), "xyz1", "typing somewhere else is a separate step");
}

#[test]
fn backspacing_groups_and_restores_in_one_undo() {
    let mut b = buf("hello world");
    at(&mut b, 0, 11);
    for _ in 0..5 {
        b.delete_backward();
    }
    assert_eq!(b.text(), "hello ");
    b.undo();
    assert_eq!(b.text(), "hello world");
    b.redo();
    assert_eq!(b.text(), "hello ");
}

#[test]
fn grouping_never_corrupts_the_saved_state() {
    let dir = std::env::temp_dir().join(format!("mide-core-{}-group", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("f.txt");
    let mut b = buf("");
    b.insert("a");
    b.save(&path).unwrap();
    assert!(!b.is_dirty());
    b.insert("b"); // must NOT fold into the step that was saved
    assert!(b.is_dirty());
    b.undo();
    assert!(!b.is_dirty());
    assert_eq!(b.text(), "a");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn select_line_grows_by_a_line_each_time() {
    let mut b = buf("one\ntwo\nthree");
    at(&mut b, 1, 1);
    b.select_line();
    assert_eq!(b.selected_text(), "two\n");
    b.select_line();
    assert_eq!(b.selected_text(), "two\nthree");
}

#[test]
fn duplicate_line_copies_below_and_moves_the_cursor_down() {
    let mut b = buf("a\nb\nc");
    at(&mut b, 1, 0);
    b.duplicate_line();
    assert_eq!(b.text(), "a\nb\nb\nc");
    assert_eq!(heads(&b), vec![(2, 0)]);

    // Last line without a trailing newline.
    let mut b = buf("a\nb");
    at(&mut b, 1, 1);
    b.duplicate_line();
    assert_eq!(b.text(), "a\nb\nb");
    assert_eq!(heads(&b), vec![(2, 1)]);

    // A multi-line selection duplicates as one block.
    let mut b = buf("1\n2\n3\n4");
    select(&mut b, (1, 0), (2, 1));
    b.duplicate_line();
    assert_eq!(b.text(), "1\n2\n3\n2\n3\n4");
    b.undo();
    assert_eq!(b.text(), "1\n2\n3\n4");
}

#[test]
fn delete_line_handles_middle_last_and_only_lines() {
    let mut b = buf("a\nb\nc");
    at(&mut b, 1, 0);
    b.delete_line();
    assert_eq!(b.text(), "a\nc");
    assert_eq!(heads(&b), vec![(1, 0)]);

    let mut b = buf("a\nb");
    at(&mut b, 1, 0);
    b.delete_line();
    assert_eq!(b.text(), "a");

    let mut b = buf("only");
    b.delete_line();
    assert_eq!(b.text(), "");

    // Two cursors on two lines delete both.
    let mut b = buf("a\nb\nc\nd");
    b.set_selections(vec![Selection::cursor(b.offset_of(0, 0)), Selection::cursor(b.offset_of(2, 0))]);
    b.delete_line();
    assert_eq!(b.text(), "b\nd");
}

#[test]
fn move_lines_swaps_with_neighbours_and_keeps_the_selection() {
    let mut b = buf("a\nb\nc\nd");
    at(&mut b, 1, 0);
    b.move_lines(true);
    assert_eq!(b.text(), "b\na\nc\nd");
    assert_eq!(heads(&b), vec![(0, 0)]);
    b.move_lines(true); // already at the top: nothing happens
    assert_eq!(b.text(), "b\na\nc\nd");

    select(&mut b, (1, 0), (2, 1)); // lines "a" and "c"... as a block
    b.move_lines(false);
    assert_eq!(b.text(), "b\nd\na\nc");
    assert_eq!(b.selected_text(), "a\nc");

    // Down at the end of the document is a no-op, and undo restores everything.
    let before = b.text();
    at(&mut b, 3, 0);
    b.move_lines(false);
    assert_eq!(b.text(), before);
    b.undo();
    assert_eq!(b.text(), "b\na\nc\nd");
}

#[test]
fn join_lines_joins_with_a_single_space() {
    let mut b = buf("foo\n    bar\nbaz");
    at(&mut b, 0, 0);
    b.join_lines();
    assert_eq!(b.text(), "foo bar\nbaz");
    assert_eq!(heads(&b), vec![(0, 3)]);

    let mut b = buf("a\nb\nc\nd");
    select(&mut b, (0, 0), (2, 1));
    b.join_lines();
    assert_eq!(b.text(), "a b c\nd");

    let mut b = buf("last");
    b.join_lines();
    assert_eq!(b.text(), "last");
}

#[test]
fn indent_and_outdent_selected_lines() {
    let mut b = buf("a\n\nb\n  c\n\td");
    select(&mut b, (0, 0), (4, 1));
    b.indent_lines(4);
    assert_eq!(b.text(), "    a\n\n    b\n      c\n    \td", "blank lines are left alone");
    b.outdent_lines(4);
    assert_eq!(b.text(), "a\n\nb\n  c\n\td");
    b.outdent_lines(4);
    assert_eq!(b.text(), "a\n\nb\nc\nd", "tabs and short indents are handled");
    assert!(b.selection_spans_lines());
}

#[test]
fn toggle_comment_adds_at_the_least_indent_and_removes() {
    let mut b = buf("fn x() {\n    a();\n\n    b();\n}");
    select(&mut b, (1, 0), (3, 8));
    b.toggle_comment("//");
    assert_eq!(b.text(), "fn x() {\n    // a();\n\n    // b();\n}");
    b.toggle_comment("//");
    assert_eq!(b.text(), "fn x() {\n    a();\n\n    b();\n}");

    // A mixed selection comments everything.
    let mut b = buf("# one\ntwo");
    select(&mut b, (0, 0), (1, 3));
    b.toggle_comment("#");
    assert_eq!(b.text(), "# # one\n# two");

    // Comment markers without a space are removed too.
    let mut b = buf("//x");
    at(&mut b, 0, 0);
    b.toggle_comment("//");
    assert_eq!(b.text(), "x");
}

#[test]
fn open_a_line_below_or_above_with_indentation() {
    let mut b = buf("  foo\nbar");
    at(&mut b, 0, 2);
    b.insert_line_after();
    assert_eq!(b.text(), "  foo\n  \nbar");
    assert_eq!(heads(&b), vec![(1, 2)]);

    let mut b = buf("  foo");
    at(&mut b, 0, 3);
    b.insert_line_before();
    assert_eq!(b.text(), "  \n  foo");
    assert_eq!(heads(&b), vec![(0, 2)]);
}

#[test]
fn add_next_occurrence_builds_multiple_cursors_and_wraps() {
    let mut b = buf("foo bar foo baz foo");
    at(&mut b, 0, 1);
    b.add_next_occurrence(); // selects the word
    assert_eq!(b.selected_text(), "foo");
    b.add_next_occurrence();
    b.add_next_occurrence();
    assert_eq!(b.selections().len(), 3);
    b.add_next_occurrence(); // everything is selected already: no change
    assert_eq!(b.selections().len(), 3);
    b.insert("X");
    assert_eq!(b.text(), "X bar X baz X");

    // Wraps around to occurrences before the cursor.
    let mut b = buf("ab ab");
    select(&mut b, (0, 3), (0, 5));
    b.add_next_occurrence();
    assert_eq!(b.selections().len(), 2);
}

#[test]
fn select_all_occurrences_of_the_word_or_selection() {
    let mut b = buf("x = x + xx");
    at(&mut b, 0, 0);
    b.select_all_occurrences();
    assert_eq!(b.selections().len(), 2, "whole word only when nothing is selected");

    let mut b = buf("x = x + xx");
    select(&mut b, (0, 0), (0, 1));
    b.select_all_occurrences();
    assert_eq!(b.selections().len(), 4, "an explicit selection matches substrings");
}

#[test]
fn goto_line_clamps() {
    let mut b = buf("one\ntwo\nthree");
    b.goto_line(2, 2);
    assert_eq!(heads(&b), vec![(1, 1)]);
    b.goto_line(99, 99);
    assert_eq!(heads(&b), vec![(2, 5)]);
    b.goto_line(0, 0);
    assert_eq!(heads(&b), vec![(0, 0)]);
}

#[test]
fn unicode_lines_survive_every_line_operation() {
    let mut b = buf("é→x\nñ");
    at(&mut b, 0, 2);
    b.duplicate_line();
    assert_eq!(b.text(), "é→x\né→x\nñ");
    b.move_lines(false);
    assert_eq!(b.text(), "é→x\nñ\né→x");
    at(&mut b, 1, 0);
    b.join_lines();
    assert_eq!(b.text(), "é→x\nñ é→x");
}
