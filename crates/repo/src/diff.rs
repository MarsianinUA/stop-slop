use camino::Utf8PathBuf;

use crate::{Change, ChangedFile, Cut, LineRange, RepoError, git};

/// One `diff --git` section; `path` is raw bytes until the whole section is read.
struct Section {
    path: Option<Vec<u8>>,
    change: Change,
    lines: Vec<LineRange>,
    hunks: Vec<Hunk>,
}

/// What a hunk takes out, for its `Cut`.
struct Hunk {
    new_side: Cut,
    /// `None` until it takes out a line of code; then whether every such line belongs to a
    /// decorator on lines of its own, such as `@IsOptional()` or `@ApiProperty({` .. `})`.
    decorators: Option<bool>,
    /// The parentheses that the decorators taken out so far leave open.
    open: usize,
}

impl Hunk {
    /// A line of a decorator starts with `@` or goes on in its open parentheses, and ends with
    /// `)` or leaves some open: `@Get(':id')`, not `@Input() name: string;`.
    fn take_out(&mut self, code: &[u8]) {
        let line = code.trim_ascii();
        let starts = self.open > 0 || line.starts_with(b"@");
        let count = |byte: u8| line.iter().filter(|&&each| each == byte).count();
        let closed = (self.open + count(b'(')).checked_sub(count(b')'));
        self.open = closed.unwrap_or(0);
        let ends = self.open > 0 || line.ends_with(b")");
        let decorator = starts && ends && closed.is_some();
        self.decorators = Some(self.decorators.unwrap_or(true) && decorator);
    }

    /// A hunk that takes out only decorators changed the code they were on: the line after it.
    fn cut(&self) -> Option<Cut> {
        let decorators = self.decorators?;
        Some(
            if self.new_side.count == 0 && decorators && self.open == 0 {
                Cut {
                    start: self.new_side.start + 1,
                    count: 1,
                }
            } else {
                self.new_side
            },
        )
    }
}

/// The output format `parse` relies on. The user's config could drop or swap the `a/` `b/`
/// prefixes, add colors, glue hunks, turn off renames or run another diff program. Another
/// algorithm would move the changed lines away from the ones a pull request on GitHub shows.
pub(crate) const ARGS: &[&str] = &[
    "-U0",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--find-renames",
    "--inter-hunk-context=0",
    "--src-prefix=a/",
    "--dst-prefix=b/",
    "--diff-algorithm=myers",
    "--indent-heuristic",
];

/// Reads `git diff` made with `ARGS`. Only lines that start with `diff --git `,
/// `new file mode `, `deleted file mode `, `rename to ` and `@@ ` count, and the removed lines
/// of a hunk. A line of a body starts with `+`, `-`, a space or `\`, so it never matches one of
/// them; `---` and `+++` come before the first hunk and are not read at all, so a removed line
/// `-- x` (printed `--- x`) cannot pass for a header. A path that is not UTF-8 is skipped.
pub(crate) fn parse(out: &[u8]) -> Result<Vec<ChangedFile>, RepoError> {
    let unexpected = || git::unexpected("diff -U0");
    let mut sections: Vec<Section> = Vec::new();
    for line in out.split(|&byte| byte == b'\n') {
        if let Some(header) = line.strip_prefix(b"diff --git ") {
            sections.push(Section {
                path: header_path(header),
                change: Change::Modified,
                lines: Vec::new(),
                hunks: Vec::new(),
            });
            continue;
        }
        let Some(section) = sections.last_mut() else {
            continue;
        };
        if line.starts_with(b"new file mode ") {
            section.change = Change::Added;
        } else if line.starts_with(b"deleted file mode ") {
            section.change = Change::Deleted;
        } else if let Some(name) = line.strip_prefix(b"rename to ") {
            section.change = Change::Renamed;
            section.path = match name.first() {
                Some(b'"') => unquote(name),
                _ => Some(name.to_vec()),
            };
        } else if let Some(hunk) = line.strip_prefix(b"@@ ") {
            let (start, count) = new_side(hunk).ok_or_else(unexpected)?;
            // `count = 0` only removes lines.
            if count > 0 {
                section.lines.push(LineRange {
                    start,
                    end: start + count - 1,
                });
            }
            section.hunks.push(Hunk {
                new_side: Cut { start, count },
                decorators: None,
                open: 0,
            });
        } else if let Some(removed) = line.strip_prefix(b"-")
            && holds_code(removed)
            && let Some(hunk) = section.hunks.last_mut()
        {
            hunk.take_out(removed);
        }
    }
    let mut files = Vec::with_capacity(sections.len());
    for section in sections {
        let path = section.path.ok_or_else(unexpected)?;
        if let Ok(path) = String::from_utf8(path) {
            files.push(ChangedFile {
                path: Utf8PathBuf::from(path),
                change: section.change,
                lines: section.lines,
                cuts: section.hunks.iter().filter_map(Hunk::cut).collect(),
            });
        }
    }
    Ok(files)
}

/// `a/P b/P` or `"a/P" "b/P"`: without a rename both halves hold the same path. With a
/// rename the halves differ and `rename to` gives the path.
fn header_path(header: &[u8]) -> Option<Vec<u8>> {
    if header.first() == Some(&b'"') {
        let name = unquote(header)?;
        return name.strip_prefix(b"a/").map(<[u8]>::to_vec);
    }
    let rest = header.strip_prefix(b"a/")?;
    let (path, other) = rest.split_at_checked(rest.len().checked_sub(3)? / 2)?;
    (other.strip_prefix(b" b/")? == path).then(|| path.to_vec())
}

/// Undoes the C quoting of git (`quote_c_style`) in the first quoted name of `text`:
/// `"src/\321\204 \"x\".ts"` -> `src/ф "x".ts`.
fn unquote(text: &[u8]) -> Option<Vec<u8>> {
    let mut bytes = text.strip_prefix(b"\"")?.iter().copied();
    let mut name = Vec::new();
    loop {
        let byte = match bytes.next()? {
            b'"' => return Some(name),
            b'\\' => match bytes.next()? {
                b'a' => 0x07,
                b'b' => 0x08,
                b't' => b'\t',
                b'n' => b'\n',
                b'v' => 0x0b,
                b'f' => 0x0c,
                b'r' => b'\r',
                escaped @ (b'"' | b'\\') => escaped,
                high @ b'0'..=b'3' => {
                    let mut octal = high - b'0';
                    for _ in 0..2 {
                        let digit = bytes.next()?.checked_sub(b'0').filter(|digit| *digit < 8)?;
                        octal = (octal << 3) | digit;
                    }
                    octal
                }
                _ => return None,
            },
            byte => byte,
        };
        name.push(byte);
    }
}

/// The old file is not parsed, so a removed line counts as a comment when, after its indent, it
/// starts with `//`, or with `/*` or `*` (inside a block comment) and holds no code after a `*/`
/// on it: `/* istanbul ignore next */ cleanup();` holds code. A guess both ways: a line `* rate`
/// of a product reads as a comment, a line of a block comment that starts otherwise as code.
fn holds_code(line: &[u8]) -> bool {
    let line = line.trim_ascii();
    if line.is_empty() || line.starts_with(b"//") {
        return false;
    }
    // Past `/*`, so that `/*/` does not close.
    let inside = match line.strip_prefix(b"/*") {
        Some(rest) => rest,
        None if line.starts_with(b"*") => line,
        None => return true,
    };
    inside
        .windows(2)
        .position(|pair| pair == b"*/")
        .is_some_and(|end| holds_code(&inside[end + 2..]))
}

/// `(c, d)` from `-a[,b] +c[,d] @@ ...`; without `,d` one line.
fn new_side(hunk: &[u8]) -> Option<(u32, u32)> {
    let new = hunk.split(|&byte| byte == b' ').nth(1)?;
    let new = str::from_utf8(new).ok()?.strip_prefix('+')?;
    let (start, count) = new.split_once(',').unwrap_or((new, "1"));
    Some((start.parse().ok()?, count.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cuts` are `(start, count)`.
    fn file(path: &str, change: Change, ranges: &[(u32, u32)], cuts: &[(u32, u32)]) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            change,
            lines: ranges
                .iter()
                .map(|&(start, end)| LineRange { start, end })
                .collect(),
            cuts: cuts
                .iter()
                .map(|&(start, count)| Cut { start, count })
                .collect(),
        }
    }

    #[test]
    fn a_quoted_name_and_a_name_with_spaces_give_the_exact_path() {
        let out = concat!(
            r#"diff --git "a/src/\321\204 \"q\"\\\t.ts" "b/src/\321\204 \"q\"\\\t.ts""#,
            "\n",
            "@@ -1 +1 @@\n",
            "-a\n",
            "+b\n",
            "diff --git a/x y.ts b/x y.ts\n",
            "@@ -1 +1 @@\n",
            "-a\n",
            "+b\n",
        );

        assert_eq!(
            parse(out.as_bytes()).unwrap(),
            [
                file("src/ф \"q\"\\\t.ts", Change::Modified, &[(1, 1)], &[(1, 1)]),
                file("x y.ts", Change::Modified, &[(1, 1)], &[(1, 1)]),
            ]
        );
    }

    #[test]
    fn a_hunk_gives_the_lines_of_the_new_side() {
        let out = concat!(
            "diff --git a/a.ts b/a.ts\n",
            "@@ -3 +3 @@\n",
            "-a\n",
            "+b\n",
            "@@ -4 +3,0 @@\n",
            "-c\n",
            "@@ -4,2 +3,0 @@\n",
            "-d\n",
            "-e\n",
            "diff --git a/b.ts b/b.ts\n",
            "new file mode 100644\n",
            "@@ -0,0 +1,5 @@\n",
            "+1\n+2\n+3\n+4\n+5\n",
        );

        assert_eq!(
            parse(out.as_bytes()).unwrap(),
            [
                file(
                    "a.ts",
                    Change::Modified,
                    &[(3, 3)],
                    &[(3, 1), (3, 0), (3, 0)]
                ),
                file("b.ts", Change::Added, &[(1, 5)], &[]),
            ]
        );
    }

    #[test]
    fn a_cut_is_a_hunk_that_takes_out_a_line_of_code() {
        let out = concat!(
            "diff --git a/a.ts b/a.ts\n",
            "@@ -2,3 +2 @@\n",
            "-  // a note\n",
            "-\n",
            "-   * of a block\n",
            "+  /** a block */\n",
            "@@ -8 +6,0 @@\n",
            "-  /* gone */\n",
            "@@ -10,2 +8,0 @@\n",
            "-\t\r\n",
            "-  return x;\r\n",
            "@@ -20 +18 @@\n",
            "-  a(); // why\n",
            "+  // a();\n",
        );

        assert_eq!(
            parse(out.as_bytes()).unwrap(),
            [file(
                "a.ts",
                Change::Modified,
                &[(2, 2), (18, 18)],
                &[(8, 0), (18, 1)]
            )]
        );
    }

    #[test]
    fn code_after_a_block_comment_on_its_line_is_code() {
        let out = concat!(
            "diff --git a/a.ts b/a.ts\n",
            "@@ -2 +1,0 @@\n",
            "-  /* istanbul ignore next */ cleanup();\n",
            "@@ -4 +2,0 @@\n",
            "-   */ }\n",
            "@@ -6 +3,0 @@\n",
            "-  /* one */ /* two */ // three\n",
            "@@ -8 +4,0 @@\n",
            "-  /*/ still a comment */\n",
        );

        assert_eq!(
            parse(out.as_bytes()).unwrap(),
            [file("a.ts", Change::Modified, &[], &[(1, 0), (2, 0)])]
        );
    }

    #[test]
    fn a_hunk_that_takes_out_only_decorators_cuts_the_line_after_it() {
        let out = concat!(
            "diff --git a/a.dto.ts b/a.dto.ts\n",
            "@@ -3,2 +2,0 @@\n",
            "-  @IsOptional()\n",
            "-  @ApiProperty({ required: false })\n",
            "@@ -9,2 +6,0 @@\n",
            "-  @Get()\n",
            "-  list() {}\n",
            "@@ -12 +8,0 @@\n",
            "-  @Input() name: string;\n",
            "@@ -14 +10 @@\n",
            "-  @Get()\n",
            "+  @Post()\n",
            "@@ -20,4 +16,0 @@\n",
            "-  @ApiProperty({\n",
            "-    description: 'price (net)',\n",
            "-    type: () => Number,\n",
            "-  })\n",
            "@@ -30 +22,0 @@\n",
            "-  @Column({\n",
        );

        assert_eq!(
            parse(out.as_bytes()).unwrap(),
            [file(
                "a.dto.ts",
                Change::Modified,
                &[(10, 10)],
                &[(3, 1), (6, 0), (8, 0), (10, 1), (17, 1), (22, 0)]
            )]
        );
    }

    #[test]
    fn sections_without_hunks_and_a_quoted_rename() {
        let out = concat!(
            "diff --git a/bin.ts b/bin.ts\n",
            "index 1234567..89abcde 100644\n",
            "Binary files a/bin.ts and b/bin.ts differ\n",
            "diff --git a/run.ts b/run.ts\n",
            "old mode 100644\n",
            "new mode 100755\n",
            "diff --git a/gone.ts b/gone.ts\n",
            "deleted file mode 100644\n",
            "index 1234567..0000000\n",
            "--- a/gone.ts\n",
            "+++ /dev/null\n",
            "@@ -1,2 +0,0 @@\n",
            "-a\n",
            "-b\n",
            r#"diff --git a/old name.ts "b/\320\275\320\276\320\262.ts""#,
            "\n",
            "similarity index 90%\n",
            "rename from old name.ts\n",
            r#"rename to "\320\275\320\276\320\262.ts""#,
            "\n",
            "@@ -4 +4 @@\n",
            "-a\n",
            "+b\n",
        );

        assert_eq!(
            parse(out.as_bytes()).unwrap(),
            [
                file("bin.ts", Change::Modified, &[], &[]),
                file("run.ts", Change::Modified, &[], &[]),
                file("gone.ts", Change::Deleted, &[], &[(0, 0)]),
                file("нов.ts", Change::Renamed, &[(4, 4)], &[(4, 1)]),
            ]
        );
    }
}
