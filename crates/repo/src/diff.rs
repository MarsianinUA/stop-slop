use camino::Utf8PathBuf;

use crate::{Change, ChangedFile, LineRange, RepoError, git};

/// One `diff --git` section; `path` is raw bytes until the whole section is read.
struct Section {
    path: Option<Vec<u8>>,
    change: Change,
    lines: Vec<LineRange>,
}

/// The output format `parse` relies on. The user's config could drop or swap the `a/` `b/`
/// prefixes, add colors, glue hunks, turn off renames or run another diff program.
pub(crate) const ARGS: &[&str] = &[
    "-U0",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--find-renames",
    "--inter-hunk-context=0",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

/// Reads `git diff` made with `ARGS`. Only lines that start with `diff --git `,
/// `new file mode `, `deleted file mode `, `rename to ` and `@@ ` count. A line of a body
/// starts with `+`, `-`, a space or `\`, so it never matches one of them; `---` and `+++`
/// are not read at all, so a removed line `-- x` (printed `--- x`) cannot pass for a
/// header. A path that is not UTF-8 is skipped.
pub(crate) fn parse(out: &[u8]) -> Result<Vec<ChangedFile>, RepoError> {
    let unexpected = || git::unexpected("diff -U0");
    let mut sections: Vec<Section> = Vec::new();
    for line in out.split(|&byte| byte == b'\n') {
        if let Some(header) = line.strip_prefix(b"diff --git ") {
            sections.push(Section {
                path: header_path(header),
                change: Change::Modified,
                lines: Vec::new(),
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

    fn file(path: &str, change: Change, ranges: &[(u32, u32)]) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            change,
            lines: ranges
                .iter()
                .map(|&(start, end)| LineRange { start, end })
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
                file("src/ф \"q\"\\\t.ts", Change::Modified, &[(1, 1)]),
                file("x y.ts", Change::Modified, &[(1, 1)]),
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
                file("a.ts", Change::Modified, &[(3, 3)]),
                file("b.ts", Change::Added, &[(1, 5)]),
            ]
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
                file("bin.ts", Change::Modified, &[]),
                file("run.ts", Change::Modified, &[]),
                file("gone.ts", Change::Deleted, &[]),
                file("нов.ts", Change::Modified, &[(4, 4)]),
            ]
        );
    }
}
