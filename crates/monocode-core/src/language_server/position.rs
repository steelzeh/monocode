//! Servers count a position as a line and a UTF-16 column. The editor uses
//! byte offsets.

pub(super) struct Lines<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    pub(super) fn new(text: &'a str) -> Self {
        let bytes = text.as_bytes();
        let mut starts = vec![0];
        for (at, byte) in bytes.iter().enumerate() {
            let ends_line = *byte == b'\n' || (*byte == b'\r' && bytes.get(at + 1) != Some(&b'\n'));
            if ends_line {
                starts.push(at + 1);
            }
        }
        Self { text, starts }
    }

    /// The byte offset of a position, kept inside its line.
    pub(super) fn byte(&self, line: usize, column: usize) -> usize {
        let Some(start) = self.starts.get(line).copied() else {
            return self.text.len();
        };
        let mut units = 0;
        for (at, ch) in self.text[start..].char_indices() {
            if units >= column || ch == '\n' || ch == '\r' {
                return start + at;
            }
            units += ch.len_utf16();
        }
        self.text.len()
    }
}

#[cfg(test)]
mod tests {
    use super::Lines;

    #[test]
    fn columns_count_utf16_units_and_the_result_is_bytes() {
        let text = "a🙂b\nøc";
        let lines = Lines::new(text);
        assert_eq!(lines.byte(0, 0), 0);
        assert_eq!(lines.byte(0, 1), 1);
        assert_eq!(lines.byte(0, 3), text.find('b').unwrap());
        assert_eq!(lines.byte(1, 1), text.find('c').unwrap());
    }
    #[test]
    fn a_column_inside_an_emoji_lands_on_a_character_boundary() {
        let text = "🙂x";
        let at = Lines::new(text).byte(0, 1);
        assert!(text.is_char_boundary(at));
    }
    #[test]
    fn positions_past_the_end_are_kept_inside_the_text() {
        let lines = Lines::new("ab\ncd");
        assert_eq!(lines.byte(0, 99), 2);
        assert_eq!(lines.byte(1, 99), 5);
        assert_eq!(lines.byte(9, 0), 5);
    }
    #[test]
    fn every_line_ending_starts_a_new_line() {
        let text = "a\r\nb\rc\nd";
        let lines = Lines::new(text);
        assert_eq!(lines.byte(0, 9), 1);
        assert_eq!(lines.byte(1, 0), text.find('b').unwrap());
        assert_eq!(lines.byte(2, 0), text.find('c').unwrap());
        assert_eq!(lines.byte(3, 0), text.find('d').unwrap());
    }
}
