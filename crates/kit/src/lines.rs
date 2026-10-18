use crate::File;

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

/// The end of the spaces, `;` and comments after `at` on its line, and whether a `;` is among
/// them; `None` when other code follows there, not a line break, the end of the text or a `}`
/// that closes the block, or when a comment there speaks of the next line
/// (`// @ts-expect-error`, `speaks_of_next_line`), which would lose its node.
pub fn line_tail(file: &File<'_>, mut at: u32) -> Option<(u32, bool)> {
    let text = file.text;
    let comments = file.semantic.comments();
    let mut semicolon = false;
    loop {
        let rest = &text[at as usize..];
        let next = at + (rest.len() - rest.trim_start_matches([' ', '\t']).len()) as u32;
        let rest = &text[next as usize..];
        if rest.starts_with(';') {
            semicolon = true;
            at = next + 1;
        } else if let Ok(index) = comments.binary_search_by_key(&next, |comment| comment.span.start)
        {
            let comment = comments[index].span;
            if speaks_of_next_line(comment.source_text(text)) {
                return None;
            }
            at = comment.end;
        } else if rest.is_empty() || rest.starts_with(['\n', '\r', '}']) {
            return Some((at, semicolon));
        } else {
            return None;
        }
    }
}

/// A directive about the line or node under the comment: `eslint-disable-next-line` and the like,
/// `@ts-expect-error`, `@ts-ignore`, `biome-ignore <rule>: ..`.
pub(crate) fn speaks_of_next_line(comment: &str) -> bool {
    [
        "-next-line",
        "@ts-expect-error",
        "@ts-ignore",
        "biome-ignore ",
    ]
    .iter()
    .any(|directive| comment.contains(directive))
}

/// A comment whose place decides what it covers beyond its own line and the next: a lint range
/// (`eslint-disable` / `eslint-enable`, the same of oxlint and tslint, `biome-ignore-start` /
/// `-end`), a file-wide `biome-ignore-all`, or a `#region` / `#endregion` fold. Code moved across
/// it changes sides; the `-line` and `-next-line` forms move with their code.
pub fn opens_region(comment: &str) -> bool {
    let ranges = [
        "eslint-disable",
        "eslint-enable",
        "oxlint-disable",
        "oxlint-enable",
        "tslint:disable",
        "tslint:enable",
    ];
    [
        "#region",
        "#endregion",
        "biome-ignore-start",
        "biome-ignore-end",
        "biome-ignore-all",
    ]
    .iter()
    .any(|marker| comment.contains(marker))
        || ranges.iter().any(|directive| {
            comment
                .split(directive)
                .skip(1)
                .any(|rest| !rest.starts_with("-line") && !rest.starts_with("-next-line"))
        })
}

/// Source text on one line for a message's `{arg}`: each run of spaces and line breaks is one
/// space, so a finding stays one line of the text output.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn line_start(text: &str, at: u32) -> u32 {
    text[..at as usize]
        .rfind('\n')
        .map_or(0, |index| index as u32 + 1)
}

/// A line of `text` other than its first and last holds only spaces: `text` is the stretch
/// between two pieces of code, so its first and last lines belong to them.
pub fn has_blank_line(text: &str) -> bool {
    let mut lines = text.split('\n');
    lines.next();
    lines.next_back();
    lines.any(|line| line.trim().is_empty())
}
