use oxc_ast::AstKind;
use oxc_ast::ast::{ImportDeclaration, ImportDeclarationSpecifier, Statement};
use oxc_semantic::SymbolId;
use oxc_span::{GetSpan, Span};

use crate::rename::{RESERVED, is_identifier, is_module};
use crate::{Edit, Facts, File, ImportKind, import_kind, line_tail};

/// A comment with one of these speaks of the line under it.
const NEXT_LINE: [&str; 3] = ["-next-line", "@ts-expect-error", "@ts-ignore"];

/// TypeScript reads these only in the comments before the first statement.
const FILE_PRAGMAS: [&str; 5] = ["<reference", "<amd-", "@ts-nocheck", "@ts-check", "@jsx"];

/// Edits that import `names` from `source` by name. `Some([])` when the file already imports
/// every name from that module, as a value and under its own name. `None` when a name is not an
/// identifier or is reserved, when another declaration or import holds a top-level name, or when
/// the file is a script: an import would make its global names local.
///
/// The rule needs `Fact::Packages` and `Fact::Aliases` in `USES`: the new line goes by
/// `import_kind`.
pub fn add_import(
    file: &File<'_>,
    facts: &dyn Facts,
    source: &str,
    names: &[&str],
) -> Option<Vec<Edit>> {
    if !is_module(file) {
        return None;
    }
    let mut missing = Vec::new();
    for &name in names {
        if !is_identifier(name) || RESERVED.contains(&name) {
            return None;
        }
        match file.semantic.scoping().get_root_binding(name.into()) {
            None if !missing.contains(&name) => missing.push(name),
            None => {}
            Some(symbol) if imports_value(file, facts, symbol, name, source) => {}
            Some(_) => return None,
        }
    }
    if missing.is_empty() {
        return Some(Vec::new());
    }
    Some(
        merged(file, facts, source, &missing)
            .unwrap_or_else(|| vec![new_line(file, facts, source, &missing)]),
    )
}

/// `symbol` is `name` itself, imported by value from the module `source`.
fn imports_value(
    file: &File<'_>,
    facts: &dyn Facts,
    symbol: SymbolId,
    name: &str,
    source: &str,
) -> bool {
    let nodes = file.semantic.nodes();
    let declaration = file.semantic.scoping().symbol_declaration(symbol);
    matches!(nodes.kind(declaration), AstKind::ImportSpecifier(specifier)
        if specifier.imported.name() == name && specifier.import_kind.is_value())
        && matches!(nodes.parent_kind(declaration), AstKind::ImportDeclaration(import)
            if import.import_kind.is_value() && same_module(file, facts, &import.source.value, source))
}

/// The same text, or one Node builtin with and without `node:`.
fn same_module(file: &File<'_>, facts: &dyn Facts, a: &str, b: &str) -> bool {
    a == b
        || a.strip_prefix("node:").unwrap_or(a) == b.strip_prefix("node:").unwrap_or(b)
            && import_kind(file, facts, a) == ImportKind::Node
            && import_kind(file, facts, b) == ImportKind::Node
}

/// The names added to the first value import of the module that has named specifiers or a lone
/// default. `import * as` has no list to join, and `import type` would bind them as types.
fn merged(file: &File<'_>, facts: &dyn Facts, source: &str, names: &[&str]) -> Option<Vec<Edit>> {
    let text = file.text;
    let program = file.semantic.nodes().program();
    program.body.iter().find_map(|statement| {
        let Statement::ImportDeclaration(import) = statement else {
            return None;
        };
        if !import.import_kind.is_value()
            || import.phase.is_some()
            || !same_module(file, facts, &import.source.value, source)
        {
            return None;
        }
        let specifiers = import.specifiers.as_ref()?.as_slice();
        let last_named = specifiers
            .iter()
            .rev()
            .find_map(|specifier| match specifier {
                ImportDeclarationSpecifier::ImportSpecifier(specifier) => Some(specifier.span),
                _ => None,
            });
        if let Some(last) = last_named {
            let close = last.end + text[last.end as usize..].find('}')? as u32;
            if !text[last.end as usize..close as usize].contains('\n') {
                return Some(vec![Edit {
                    span: Span::empty(last.end),
                    text: names.iter().map(|name| format!(", {name}")).collect(),
                }]);
            }
            return Some(merged_lines(file, last, close, names));
        }
        let [ImportDeclarationSpecifier::ImportDefaultSpecifier(default)] = specifiers else {
            return None;
        };
        Some(vec![Edit {
            span: Span::empty(default.span.end),
            text: format!(", {{ {} }}", names.join(", ")),
        }])
    })
}

/// The names as lines of a list with one specifier a line, above its `}` with the indent of the
/// `last` specifier, so a comment after `last` stays with it. `last` gets a `,` when the list has
/// no trailing comma, and the new last name goes without one.
fn merged_lines(file: &File<'_>, last: Span, close: u32, names: &[&str]) -> Vec<Edit> {
    let text = file.text;
    let mut code = String::new();
    let mut at = last.end;
    for comment in file.semantic.comments_range(last.end..close) {
        code.push_str(&text[at as usize..comment.span.start as usize]);
        at = comment.span.end;
    }
    code.push_str(&text[at as usize..close as usize]);
    let trailing_comma = code.contains(',');

    let line = &text[line_start(text, last.start) as usize..last.start as usize];
    let indent = &line[..line.len() - line.trim_start().len()];
    let lines: String = names
        .iter()
        .map(|name| format!("{indent}{name},\n"))
        .collect();
    let lines = if trailing_comma {
        lines
    } else {
        format!("{}\n", lines.trim_end_matches(",\n"))
    };
    let mut edits = Vec::new();
    if !trailing_comma {
        edits.push(Edit {
            span: Span::empty(last.end),
            text: ",".to_string(),
        });
    }
    edits.push(Edit {
        span: Span::empty(line_above(file, close, |_| false)),
        text: lines,
    });
    edits
}

/// A new import line. In the last block of imports with bindings it goes after the last import
/// of a group not after its own, else above the first import, so `import-order` finds it in
/// place. Without a block it goes after the last import, else above the first statement.
fn new_line(file: &File<'_>, facts: &dyn Facts, source: &str, names: &[&str]) -> Edit {
    let text = file.text;
    let program = file.semantic.nodes().program();
    let own = group(import_kind(file, facts, source));
    let line = |neighbour: Option<&ImportDeclaration<'_>>| {
        let (quote, semicolon) = neighbour.map_or(('\'', true), |import| {
            (
                char::from(text.as_bytes()[import.source.span.start as usize]),
                import.span.source_text(text).ends_with(';'),
            )
        });
        let semicolon = if semicolon { ";" } else { "" };
        let names = names.join(", ");
        format!("import {{ {names} }} from {quote}{source}{quote}{semicolon}")
    };

    let block = program
        .body
        .chunk_by(|a, b| with_bindings(a).is_some() && with_bindings(b).is_some())
        .rev()
        .find_map(|run| run.iter().map(with_bindings).collect::<Option<Vec<_>>>());
    if let Some(block) = block {
        let groups: Vec<u8> = block
            .iter()
            .map(|import| group(import_kind(file, facts, &import.source.value)))
            .collect();
        let spaced = block.windows(2).any(|pair| {
            has_blank_line(&text[pair[0].span.end as usize..pair[1].span.start as usize])
        });
        let blank = |other: u8| if spaced && other != own { "\n" } else { "" };
        return match groups.iter().rposition(|&group| group <= own) {
            Some(index) => Edit {
                span: Span::empty(line_end(file, block[index])),
                text: format!("\n{}{}", blank(groups[index]), line(Some(block[index]))),
            },
            None => Edit {
                span: Span::empty(line_above(file, block[0].span.start, |comment| {
                    NEXT_LINE
                        .iter()
                        .any(|directive| comment.contains(directive))
                })),
                text: format!("{}\n{}", line(Some(block[0])), blank(groups[0])),
            },
        };
    }

    let last_import = program
        .body
        .iter()
        .rev()
        .find_map(|statement| match statement {
            Statement::ImportDeclaration(import) => Some(&**import),
            _ => None,
        });
    match last_import {
        Some(import) => Edit {
            span: Span::empty(line_end(file, import)),
            text: format!("\n{}", line(Some(import))),
        },
        // A module has a statement: an import or an export.
        None => Edit {
            span: Span::empty(line_above(file, program.body[0].span().start, |comment| {
                !FILE_PRAGMAS.iter().any(|pragma| comment.contains(pragma))
            })),
            text: format!("{}\n\n", line(None)),
        },
    }
}

/// Same groups as the `import-order` rule.
fn group(kind: ImportKind) -> u8 {
    match kind {
        ImportKind::Node => 0,
        ImportKind::External => 1,
        ImportKind::Workspace | ImportKind::Alias => 2,
        ImportKind::Relative => 3,
    }
}

/// An import `import-order` sorts. `import 'x'`, `import {} from 'x'` and `import defer` /
/// `import source` keep their place and end a block.
fn with_bindings<'a>(statement: &'a Statement<'a>) -> Option<&'a ImportDeclaration<'a>> {
    match statement {
        Statement::ImportDeclaration(import)
            if import.phase.is_none()
                && import
                    .specifiers
                    .as_ref()
                    .is_some_and(|list| !list.is_empty()) =>
        {
            Some(import)
        }
        _ => None,
    }
}

/// After the import and the rest of its line; right after the import when code or a comment
/// about the next line follows it there.
fn line_end(file: &File<'_>, import: &ImportDeclaration<'_>) -> u32 {
    line_tail(file, import.span.end).map_or(import.span.end, |(end, _)| end)
}

/// Where a line goes above `start`: the start of its line, over the comments right above it that
/// `lift` takes; `start` itself when code comes before it on its line.
fn line_above(file: &File<'_>, start: u32, lift: impl Fn(&str) -> bool) -> u32 {
    let text = file.text;
    let mut at = line_start(text, start);
    if !text[at as usize..start as usize].trim().is_empty() {
        return start;
    }
    for comment in file.semantic.comments_range(..at).rev() {
        let line = line_start(text, comment.span.start);
        let gap = &text[comment.span.end as usize..at as usize];
        if !lift(comment.span.source_text(text))
            || !gap.trim().is_empty()
            || has_blank_line(gap)
            || !text[line as usize..comment.span.start as usize]
                .trim()
                .is_empty()
        {
            break;
        }
        at = line;
    }
    at
}

/// A line of `text` other than its first and last holds only spaces.
fn has_blank_line(text: &str) -> bool {
    let mut lines = text.split('\n');
    lines.next();
    lines.next_back();
    lines.any(|line| line.trim().is_empty())
}

fn line_start(text: &str, at: u32) -> u32 {
    text[..at as usize]
        .rfind('\n')
        .map_or(0, |index| index as u32 + 1)
}
