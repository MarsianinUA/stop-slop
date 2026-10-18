use std::collections::BTreeSet;

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
        for block in comment_blocks(file) {
            // `/* force */` between tokens labels the code after it.
            if has_code_after(file.text, block.span) {
                continue;
            }
            for (first, last) in sentences(file.text, &block) {
                let start = first.source_text(file.text).trim();
                let starts = starts_as_sentence(file, dependencies, start);
                let ends = ends_as_sentence(last.source_text(file.text).trim());
                let problem = match (starts, ends) {
                    (true, true) => continue,
                    (false, true) => "starts with a lowercase letter",
                    (true, false) => "does not end with a period",
                    (false, false) => {
                        "starts with a lowercase letter and does not end with a period"
                    }
                };
                let body = first.source_text(file.text);
                let start = first.start + (body.len() - body.trim_start().len()) as u32;
                cx.report(Span::new(start, last.end))
                    .arg("problem", problem);
            }
        }
    }
}

fn has_code_after(text: &str, span: Span) -> bool {
    let rest = &text[span.end as usize..];
    !rest[..rest.find('\n').unwrap_or(rest.len())]
        .trim()
        .is_empty()
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
    let base = block
        .parts
        .iter()
        .flat_map(|part| &part.lines)
        .map(|&line| indent(text, line))
        .min()
        .unwrap_or(0);
    let mut sentences = Vec::new();
    let mut fenced = false;
    let mut introduced = false;
    for part in &block.parts {
        let mut skipped = part.kind != CommentKind::Prose
            || indent(text, part.lines[0]) > base
            || is_list_item(part.lines[0].source_text(text).trim());
        let mut sentence: Option<(Span, Span)> = None;
        for &line in &part.lines {
            let body = line.source_text(text).trim();
            let fence = body.starts_with("```") || body.starts_with("~~~");
            fenced ^= fence;
            introduced |= body.starts_with('@') && body[1..].starts_with(char::is_alphabetic);
            skipped |= body.starts_with('|') || is_todo(body);
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
fn starts_as_sentence(
    file: &File<'_>,
    dependencies: Option<&BTreeSet<String>>,
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
        || is_name_in_file(file, word)
}

fn is_name_in_file(file: &File<'_>, word: &str) -> bool {
    file.semantic.nodes().iter().any(|node| match node.kind() {
        AstKind::IdentifierName(name) => name.name == word,
        AstKind::IdentifierReference(name) => name.name == word,
        AstKind::BindingIdentifier(name) => name.name == word,
        AstKind::StringLiteral(literal) => {
            literal.value == word || package_name(&literal.value) == word
        }
        _ => false,
    })
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
