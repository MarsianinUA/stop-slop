use std::cell::LazyCell;
use std::collections::HashSet;

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
        // Collected on the first sentence that needs them: a lookup per sentence would walk every
        // node again.
        let names = LazyCell::new(|| names_in_file(file));
        let known = |word: &str| {
            dependencies.is_some_and(|packages| packages.contains(word)) || names.contains(word)
        };
        for block in comment_blocks(file) {
            if is_inside_code(file, block.span) {
                continue;
            }
            for (first, last) in sentences(file.text, &block) {
                let starts = starts_as_sentence(first.source_text(file.text).trim(), known);
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

/// A `/* */` between the tokens of a line labels the code next to it: `f(/* force */ true)`,
/// `catch { /* ignore */ }`. A doc comment before a declaration on its line and a note after the
/// code are comments of their own.
fn is_inside_code(file: &File<'_>, span: Span) -> bool {
    let before = &file.text[line_start(file.text, span.start) as usize..span.start as usize];
    !before.trim().is_empty()
        && line_tail(file, span.end).is_none_or(|(end, _)| {
            file.text[end as usize..]
                .trim_start_matches([' ', '\t'])
                .starts_with('}')
        })
}

fn indent(text: &str, line: Span) -> usize {
    let body = line.source_text(text);
    body.len() - body.trim_start().len()
}

/// Where the line's text stands on its line of the file: the lines of a block line up by it,
/// whether they follow `//`, a ` * ` gutter or the `/*` itself.
fn column(text: &str, line: Span) -> usize {
    (line.start - line_start(text, line.start)) as usize + indent(text, line)
}

/// The first and last line of each sentence in the block. Not sentences: code and directives,
/// a `TODO` and the rest of its paragraph, everything after a line that ends with `:`
/// (`Example:`, `Steps:` introduce what follows), a paragraph indented two columns or more
/// deeper than the block (a listing), a list, a table, a fenced example, a Markdown heading, doc
/// tags (`@param`) and the rest of the block after them, a lone inline tag (`{@inheritDoc}`),
/// and lines without letters or digits (`// -----` over and under a title).
fn sentences(text: &str, block: &CommentBlock) -> Vec<(Span, Span)> {
    // Commented-out code keeps the indent of the code, not of the text around it.
    let base = block
        .parts
        .iter()
        .filter(|part| part.kind == CommentKind::Prose)
        .flat_map(|part| &part.lines)
        .map(|&line| column(text, line))
        .min()
        .unwrap_or(0);
    let mut sentences = Vec::new();
    let mut fenced = false;
    let mut introduced = false;
    for part in &block.parts {
        // One column deeper is a habit of the writer (` *  more`), not a listing.
        let listing = part.kind != CommentKind::Prose || column(text, part.lines[0]) > base + 1;
        let mut skipped = listing;
        let mut sentence: Option<(Span, Span)> = None;
        for &line in &part.lines {
            let body = line.source_text(text).trim();
            let fence = body.starts_with("```") || body.starts_with("~~~");
            fenced ^= fence;
            // A decorator in commented-out code or in an example is no doc tag.
            introduced |= !listing && !fence && !fenced && is_tag(body);
            skipped |= body.starts_with('|') || is_todo(body) || is_list_item(body);
            let aside = !body.contains(char::is_alphanumeric)
                || body.starts_with("{@") && body.ends_with('}')
                || is_heading(body);
            if fence || fenced || introduced || skipped || aside {
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

/// `@param`, `@returns`.
fn is_tag(body: &str) -> bool {
    body.strip_prefix('@')
        .is_some_and(|rest| rest.starts_with(char::is_alphabetic))
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
/// literal (`null`), or a `known` name (the file declares or imports it, a dependency: `viem`).
fn starts_as_sentence(body: &str, known: impl Fn(&str) -> bool) -> bool {
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
        || known(word)
}

/// The names the file declares or imports, and the modules it imports. A member the file only
/// reads or calls (`cache.get(..)`) is not one of them: `get`, `set` and `map` are words as well.
fn names_in_file<'a>(file: &File<'a>) -> HashSet<&'a str> {
    let nodes = file.semantic.nodes();
    nodes
        .iter()
        .flat_map(|node| match node.kind() {
            AstKind::BindingIdentifier(name) => [Some(name.name.as_str()), None],
            AstKind::IdentifierName(name) if declares(nodes.parent_kind(node.id())) => {
                [Some(name.name.as_str()), None]
            }
            kind => match module_source(file, kind) {
                Some(source) => {
                    let value = source.value.as_str();
                    [Some(value), Some(package_name(value))]
                }
                None => [None, None],
            },
        })
        .flatten()
        .collect()
}

/// The parent of a name declares it: a key of an object, a class, an interface or an enum, or a
/// name an import or an export lists.
fn declares(parent: AstKind<'_>) -> bool {
    matches!(
        parent,
        AstKind::ObjectProperty(_)
            | AstKind::PropertyDefinition(_)
            | AstKind::MethodDefinition(_)
            | AstKind::AccessorProperty(_)
            | AstKind::TSPropertySignature(_)
            | AstKind::TSMethodSignature(_)
            | AstKind::TSEnumMember(_)
            | AstKind::ImportSpecifier(_)
            | AstKind::ExportSpecifier(_)
    )
}

/// `.`, `?`, `!`, `:` or `…` (and `。`, `？`, `！`), also before a closing quote, bracket or
/// backtick; or a link, which a period would break.
fn ends_as_sentence(body: &str) -> bool {
    let last_word = body.rsplit(char::is_whitespace).next().unwrap_or("");
    let link = last_word.trim_start_matches(['(', '<', '[']);
    link.starts_with("https://")
        || link.starts_with("http://")
        || body
            .trim_end_matches([
                ')', ']', '"', '\'', '`', '*', '_', '”', '’', '»', '」', '』',
            ])
            .ends_with(['.', '?', '!', ':', '…', '。', '？', '！'])
}
