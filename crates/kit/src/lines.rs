#[derive(Debug)]
pub struct Position {
    pub line: u32,
    /// In chars, not bytes.
    pub column: u32,
}

#[derive(Debug)]
pub struct LineIndex {
    line_starts: Vec<u32>,
    /// Byte offset just past each non-ASCII char and the running total of extra UTF-8 bytes
    /// (length minus one) of every non-ASCII char up to and including it. Keyed by the end,
    /// so an offset inside a char does not count that char and the column cannot underflow.
    wide_chars: Vec<(u32, u32)>,
    line_ending: &'static str,
}

impl LineIndex {
    pub fn new(text: &str) -> LineIndex {
        let mut line_starts = vec![0];
        let mut wide_chars = Vec::new();
        let mut extra = 0;
        for (offset, ch) in text.char_indices() {
            if ch == '\n' {
                line_starts.push(offset as u32 + 1);
            } else if !ch.is_ascii() {
                extra += ch.len_utf8() as u32 - 1;
                wide_chars.push(((offset + ch.len_utf8()) as u32, extra));
            }
        }
        let line_ending = match line_starts.get(1) {
            Some(&next) if text[..next as usize - 1].ends_with('\r') => "\r\n",
            _ => "\n",
        };
        LineIndex {
            line_starts,
            wide_chars,
            line_ending,
        }
    }

    /// `offset` is in bytes, like a `Span`; line and column start at 1.
    pub fn position(&self, offset: u32) -> Position {
        let line = self.line_starts.partition_point(|&start| start <= offset);
        let start = self.line_starts[line - 1];
        let extra = self.extra_before(offset) - self.extra_before(start);
        Position {
            line: line as u32,
            column: offset - start - extra + 1,
        }
    }

    fn extra_before(&self, offset: u32) -> u32 {
        match self.wide_chars.partition_point(|&(end, _)| end <= offset) {
            0 => 0,
            n => self.wide_chars[n - 1].1,
        }
    }

    /// The first line ending of the text; `"\n"` when there is none.
    pub fn line_ending(&self) -> &'static str {
        self.line_ending
    }
}
