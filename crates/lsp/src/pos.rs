//! Columns. LSP counts UTF-16 code units; mide counts `char`s. A line with an emoji has them differ.

/// The UTF-16 column of char column `col` in `line`.
pub fn char_to_utf16(line: &str, col: usize) -> usize {
    line.chars().take(col).map(char::len_utf16).sum()
}

/// The char column for UTF-16 column `utf16` in `line`; a column inside a surrogate pair rounds down,
/// one past the end clamps to the line length.
pub fn utf16_to_char(line: &str, utf16: usize) -> usize {
    let (mut units, mut chars) = (0, 0);
    for c in line.chars() {
        if units >= utf16 {
            break;
        }
        units += c.len_utf16();
        if units > utf16 {
            break; // inside a pair: stay before it
        }
        chars += 1;
    }
    chars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_and_bmp_are_one_unit_per_char() {
        assert_eq!(char_to_utf16("héllo→x", 5), 5);
        assert_eq!(utf16_to_char("héllo→x", 6), 6);
    }

    #[test]
    fn astral_chars_take_two_units() {
        let line = "a😀b";
        assert_eq!(char_to_utf16(line, 1), 1);
        assert_eq!(char_to_utf16(line, 2), 3);
        assert_eq!(char_to_utf16(line, 3), 4);
        assert_eq!(utf16_to_char(line, 3), 2);
        assert_eq!(utf16_to_char(line, 2), 1, "inside the pair rounds down");
        assert_eq!(utf16_to_char(line, 99), 3, "past the end clamps");
    }
}
