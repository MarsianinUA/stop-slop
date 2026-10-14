use std::fs;
use std::path::Path;

use camino::Utf8PathBuf;

use crate::error::io;
use crate::{FolderError, Problem};

#[derive(Debug)]
pub struct Fixture {
    /// From `// @path`, else `src/fixture.ts`.
    pub path: Utf8PathBuf,
    /// From `// @alias` headers.
    pub aliases: Vec<String>,
    /// From `// @workspace` headers.
    pub workspace: Vec<String>,
    /// From `// @dependency` headers.
    pub dependencies: Vec<String>,
    /// Header lines become empty lines and `// expect:` tails are cut with the spaces
    /// before them, so line numbers stay the same.
    pub text: String,
    pub expected: Vec<Expected>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Expected {
    pub line: u32,
    pub rule: String,
    pub count: u32,
}

const EXPECT: &str = "// expect:";

/// Headers are the run of `// @path`, `// @alias`, `// @workspace`, `// @dependency` lines at
/// the top of the file; any other line ends the run, `// @ts-nocheck` too. A mark is a line
/// tail `// expect: <id>` or `// expect: <id> x<count>`.
pub fn read_fixture(file: &Path) -> Result<Fixture, FolderError> {
    let error = |problem| FolderError::new(None, file, problem);
    let source = fs::read_to_string(file).map_err(|e| error(io(e)))?;
    // An editor's BOM would hide the first header; the engine drops it before parsing anyway.
    let source = source.strip_prefix('\u{feff}').unwrap_or(&source);

    let mut fixture = Fixture {
        path: Utf8PathBuf::from("src/fixture.ts"),
        aliases: Vec::new(),
        workspace: Vec::new(),
        dependencies: Vec::new(),
        text: String::with_capacity(source.len()),
        expected: Vec::new(),
    };
    let mut in_headers = true;
    for (index, line) in source.split_inclusive('\n').enumerate() {
        let number = index as u32 + 1;
        let body = line.trim_end_matches(['\r', '\n']);
        let ending = &line[body.len()..];

        if in_headers {
            if let Some((name, value)) = header(body) {
                if value.is_empty() {
                    return Err(error(Problem::BadFixture(format!(
                        "line {number}: `// @{name}` needs a value"
                    ))));
                }
                match name {
                    "path" => fixture.path = value.into(),
                    "alias" => fixture.aliases.push(value.to_string()),
                    "dependency" => fixture.dependencies.push(value.to_string()),
                    _ => fixture.workspace.push(value.to_string()),
                }
                fixture.text.push_str(ending);
                continue;
            }
            in_headers = false;
        }

        // The last match, so `// expect:` inside a string earlier on the line stays code.
        let Some(at) = body.rfind(EXPECT) else {
            fixture.text.push_str(line);
            continue;
        };
        let expected = expect(number, &body[at + EXPECT.len()..]).map_err(error)?;
        fixture.expected.push(expected);
        fixture.text.push_str(body[..at].trim_end());
        fixture.text.push_str(ending);
    }
    Ok(fixture)
}

/// `(name, value)` of a `// @path`, `// @alias`, `// @workspace` or `// @dependency` line.
fn header(body: &str) -> Option<(&str, &str)> {
    let rest = body.strip_prefix("// @")?;
    let (name, value) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    matches!(name, "path" | "alias" | "workspace" | "dependency").then_some((name, value.trim()))
}

fn expect(line: u32, mark: &str) -> Result<Expected, Problem> {
    let bad = |what: String| Problem::BadFixture(format!("line {line}: {what}"));
    let mut words = mark.split_whitespace();
    let rule = words
        .next()
        .ok_or_else(|| bad(format!("`{EXPECT}` needs a rule id")))?;
    let count = match words.next() {
        None => 1,
        Some(word) => word
            .strip_prefix('x')
            .and_then(|n| n.parse().ok())
            .filter(|&n| n > 0)
            .ok_or_else(|| bad(format!("`{word}` is not a count like `x2`")))?,
    };
    if let Some(extra) = words.next() {
        return Err(bad(format!("unexpected `{extra}` after the count")));
    }
    Ok(Expected {
        line,
        rule: rule.to_string(),
        count,
    })
}
