use std::collections::{BTreeSet, HashSet};

use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const USES: &'static [Fact] = &[Fact::Packages];

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let dependencies = cx
            .facts()
            .package(file.path)
            .map(|package| &package.dependencies);
        let mut names = None;
        for block in comment_blocks(file) {
            // `/* force */` between tokens labels the code after it.
            if is_inside_code(file, block.span) {
                continue;
            }
            for (first, last) in sentences(file.text, &block) {
                let opening = first.source_text(file.text).trim();
                let starts = starts_as_sentence(file, dependencies, &mut names, opening);
                let ends = ends_as_sentence(last.source_text(file.text).trim());
                let problem = match (starts, ends) {
                    (true, true) => continue,
                    (false, true) => "starts with a lowercase letter",
                    (true, false) => "does not end with a period",
                    (false, false) => {
                        "starts with a lowercase letter and does not end with a period"
                    }
                };
                let start = first.start + indent(file.text, first) as u32;
                cx.report(Span::new(start, last.end))
                    .arg("problem", problem);
            }
        }
    }
}

/// A doc comment before a declaration on its line and a note after the code (with other comments
/// after it) are judged.
fn is_inside_code(file: &File<'_>, span: Span) -> bool {
    let before = &file.text[line_start(file.text, span.start) as usize..span.start as usize];
    !before.trim().is_empty() && line_tail(file, span.end).is_none()
}

fn indent(text: &str, line: Span) -> usize {
    let body = line.source_text(text);
    body.len() - body.trim_start().len()
}

/// The first and last line of each sentence in the block. Not sentences: code and directives,
/// a `TODO` and the rest of its paragraph, everything after a line that ends with `:`
/// (`Example:`, `Steps:` introduce what follows), a paragraph indented deeper than the block
/// (a listing), a list, a table, a fenced example, a Markdown heading, doc tags (`@param`) and
/// the rest of the block after them, and text without letters (`// -----`).
fn sentences(text: &str, block: &CommentBlock) -> Vec<(Span, Span)> {
    // Commented-out code keeps the indent of the code, not of the text around it.
    let base = block
        .parts
        .iter()
        .filter(|part| part.kind == CommentKind::Prose)
        .flat_map(|part| &part.lines)
        .map(|&line| indent(text, line))
        .min()
        .unwrap_or(0);
    let mut sentences = Vec::new();
    let mut fenced = false;
    let mut introduced = false;
    for part in &block.parts {
        let mut skipped = part.kind != CommentKind::Prose || indent(text, part.lines[0]) > base;
        let mut sentence: Option<(Span, Span)> = None;
        for &line in &part.lines {
            let body = line.source_text(text).trim();
            let fence = body.starts_with("```") || body.starts_with("~~~");
            fenced ^= fence;
            introduced |= body.starts_with('@') && body[1..].starts_with(char::is_alphabetic);
            skipped |= body.starts_with('|') || is_todo(body) || is_list_item(body);
            if fence || fenced || introduced || skipped || is_heading(body) {
                push(&mut sentences, sentence.take(), text);
                continue;
            }
            sentence = Some((sentence.map_or(line, |(first, _)| first), line));
            introduced = body.ends_with(':');
        }
        push(&mut sentences, sentence, text);
    }
    sentences
}

fn push(sentences: &mut Vec<(Span, Span)>, sentence: Option<(Span, Span)>, text: &str) {
    if let Some((first, last)) = sentence
        && Span::new(first.start, last.end)
            .source_text(text)
            .contains(char::is_alphabetic)
    {
        sentences.push((first, last));
    }
}

/// `## Example`.
fn is_heading(body: &str) -> bool {
    let rest = body.trim_start_matches('#');
    rest.len() < body.len() && rest.starts_with(' ')
}

fn is_todo(body: &str) -> bool {
    body.strip_prefix("TODO")
        .is_some_and(|rest| !rest.starts_with(|ch: char| ch.is_alphanumeric() || ch == '_'))
}

/// `- item`, `* item`, `1. item`, `2) item`.
fn is_list_item(body: &str) -> bool {
    let rest = body.trim_start_matches(|ch: char| ch.is_ascii_digit());
    let rest = if rest.len() < body.len() {
        rest.strip_prefix(['.', ')'])
    } else {
        rest.strip_prefix(['-', '*', '+', '•'])
    };
    rest.is_some_and(|rest| rest.starts_with(' '))
}

/// A capital letter, a character with no case (`2^53`, `` `x` ``, `$1`), or a name from the code
/// as it is written: one only code would spell (`outboxSeq`, `node-redis`, `base64url`), a
/// literal (`null`), a name the file uses, or a package the file can import (`viem`).
fn starts_as_sentence<'a>(
    file: &File<'a>,
    dependencies: Option<&BTreeSet<String>>,
    names: &mut Option<HashSet<&'a str>>,
    body: &str,
) -> bool {
    if !body.starts_with(char::is_lowercase) {
        return true;
    }
    let word = body
        .split(char::is_whitespace)
        .next()
        .unwrap_or("")
        .trim_end_matches([',', '.', ';', ':', '!', '?', ')']);
    word.chars()
        .any(|ch| !ch.is_lowercase() && ch != '\'' && ch != '’')
        || matches!(word, "null" | "undefined" | "true" | "false")
        || dependencies.is_some_and(|names| names.contains(word))
        || names
            .get_or_insert_with(|| names_in_file(file))
            .contains(word)
}

/// Collected once per file, on the first sentence that needs it: a lookup per sentence would walk
/// every node again.
fn names_in_file<'a>(file: &File<'a>) -> HashSet<&'a str> {
    file.semantic
        .nodes()
        .iter()
        .flat_map(|node| match node.kind() {
            AstKind::IdentifierName(name) => [Some(name.name.as_str()), None],
            AstKind::IdentifierReference(name) => [Some(name.name.as_str()), None],
            AstKind::BindingIdentifier(name) => [Some(name.name.as_str()), None],
            AstKind::StringLiteral(literal) => {
                let value = literal.value.as_str();
                [Some(value), Some(package_name(value))]
            }
            _ => [None, None],
        })
        .flatten()
        .collect()
}

/// `.`, `?`, `!`, `:` or `…`, also before a closing quote, bracket or backtick; or a link,
/// which a period would break.
fn ends_as_sentence(body: &str) -> bool {
    let last_word = body.rsplit(char::is_whitespace).next().unwrap_or("");
    let link = last_word.trim_start_matches(['(', '<', '[']);
    link.starts_with("https://")
        || link.starts_with("http://")
        || body
            .trim_end_matches([')', ']', '"', '\'', '`', '*', '_', '”', '’', '»'])
            .ends_with(['.', '?', '!', ':', '…'])
}
