use oxc_allocator::Allocator;
use oxc_ast::Comment;
use oxc_ast::ast::{ClassElement, Expression, ObjectPropertyKind, Program, Statement};
use oxc_parser::{ParseOptions, Parser};
use oxc_span::{GetSpan, SourceType, Span};

use crate::{File, has_blank_line, line_start};

/// What a stretch of comment text is to a reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    Prose,
    /// Lines that parse as TypeScript statements, class members, object properties or
    /// decorators, outside a Markdown fence.
    Code,
    /// A note to a tool: `eslint-disable-next-line`, `@ts-expect-error`, `prettier-ignore`, `#region`.
    Directive,
}

/// One comment, or a run of `//` comments on their own consecutive lines, cut into parts.
#[derive(Debug)]
pub struct CommentBlock {
    /// The comments with their markers.
    pub span: Span,
    /// A `/** */` doc comment.
    pub doc: bool,
    pub parts: Vec<CommentPart>,
}

/// A paragraph of prose, a run of code lines, or a directive.
#[derive(Debug)]
pub struct CommentPart {
    pub kind: CommentKind,
    /// From the first character of the first line to the end of the last line.
    pub span: Span,
    /// Each non-blank line after its marker (`//`, `/*`, the ` * ` gutter) and without trailing
    /// spaces; the indentation after the marker stays.
    pub lines: Vec<Span>,
}

/// The comments of the file in text order. `//` comments, each alone on its line, on consecutive
/// lines are one block; any other comment (`/* */`, `/** */`, `//` after code) is a block of its
/// own. A directive is a block of its own too, so it also ends a run of `//` comments. In a block,
/// the longest runs of lines that parse as code are `Code`, and blank lines cut the rest into
/// `Prose` paragraphs. Blank lines between `//` comments stay in the block, as an editor leaves
/// them in commented-out code. A fenced example (```` ``` ````) is prose, and a run of code does not
/// start at or pass a line that ends with `.`: that line ends a sentence.
pub fn comment_blocks(file: &File<'_>) -> Vec<CommentBlock> {
    let text = file.text;
    let comments = file.semantic.comments();
    let mut parser = CodeParser::new(file.semantic.source_type().is_jsx());
    let mut blocks = Vec::new();
    let mut index = 0;
    while index < comments.len() {
        let first = &comments[index];
        let mut lines = comment_lines(text, first);
        let mut end = index + 1;
        let directive = is_directive(text, first);
        if !directive && first.is_line() && is_own_line(text, first.span) {
            while let Some(next) = comments.get(end)
                && next.is_line()
                && is_own_line(text, next.span)
                && !is_directive(text, next)
                && let Some(gap) = blank_gap(text, comments[end - 1].span.end, next.span.start)
            {
                if gap {
                    lines.push(Span::empty(next.span.start));
                }
                lines.extend(comment_lines(text, next));
                end += 1;
            }
        }
        let parts = if directive {
            lines.retain(|line| !line.is_empty());
            let mut parts = Vec::new();
            flush(&mut parts, &mut lines, CommentKind::Directive, text);
            parts
        } else {
            parts(&mut parser, text, &lines)
        };
        blocks.push(CommentBlock {
            span: Span::new(first.span.start, comments[end - 1].span.end),
            doc: !directive && first.is_jsdoc(),
            parts,
        });
        index = end;
    }
    blocks
}

/// The text of each line of the comment after its marker, without trailing spaces: an empty span
/// for a blank line.
fn comment_lines(text: &str, comment: &Comment) -> Vec<Span> {
    let content = comment.content_span();
    if comment.is_line() {
        // `///` is a directive or a doc line; its extra slashes are part of the marker.
        let body = content.source_text(text);
        let start = content.start + (body.len() - body.trim_start_matches('/').len()) as u32;
        return vec![trim_end(text, Span::new(start, content.end))];
    }
    let mut lines = Vec::new();
    let mut start = content.start;
    for line in content.source_text(text).split('\n') {
        let indent = line.len() - line.trim_start().len();
        let after_gutter = if line[indent..].starts_with('*') {
            indent + 1
        } else {
            0
        };
        let span = Span::new(start + after_gutter as u32, start + line.len() as u32);
        lines.push(trim_end(text, span));
        start += line.len() as u32 + 1;
    }
    lines
}

fn trim_end(text: &str, span: Span) -> Span {
    Span::sized(span.start, span.source_text(text).trim_end().len() as u32)
}

fn is_own_line(text: &str, span: Span) -> bool {
    let before = &text[line_start(text, span.start) as usize..span.start as usize];
    let after = &text[span.end as usize..];
    let line_end = after.find('\n').unwrap_or(after.len());
    before.trim().is_empty() && after[..line_end].trim().is_empty()
}

/// `Some(has a blank line)` when only spaces and line breaks part two comments.
fn blank_gap(text: &str, end: u32, start: u32) -> Option<bool> {
    let gap = &text[end as usize..start as usize];
    gap.trim().is_empty().then(|| has_blank_line(gap))
}

/// Markers a tool reads at the start of a comment. oxc already marks `/*! */`, `@license`,
/// `#__PURE__`, webpack and Vite hints and coverage ignores (`istanbul ignore next`).
const DIRECTIVES: &[&str] = &[
    "eslint-disable",
    "eslint-enable",
    "oxlint-disable",
    "oxlint-enable",
    "tslint:",
    "biome-ignore",
    "prettier-ignore",
    "dprint-ignore",
    "deno-lint-ignore",
    "deno-fmt-ignore",
    "@ts-",
    "#region",
    "#endregion",
    "<reference ",
    "<amd-",
    "@jsx",
    "cspell:",
    "spell-checker:",
    "noinspection",
    "type-coverage:",
    "@formatter:",
    "# sourceMappingURL=",
    "@ sourceMappingURL=",
];

/// Markers ESLint and JSHint read only in a `/* */` comment: `// global state` is prose.
const BLOCK_DIRECTIVES: &[&str] = &[
    "eslint ",
    "eslint-env ",
    "global ",
    "globals ",
    "exported ",
    "jshint ",
    "jslint ",
];

fn is_directive(text: &str, comment: &Comment) -> bool {
    if comment.is_annotation() || comment.content == oxc_ast::CommentContent::Legal {
        return true;
    }
    let content = comment
        .content_span()
        .source_text(text)
        .trim_start_matches(|ch: char| ch == '*' || ch == '/' || ch.is_whitespace());
    DIRECTIVES.iter().any(|marker| content.starts_with(marker))
        || comment.is_block()
            && BLOCK_DIRECTIVES
                .iter()
                .any(|marker| content.starts_with(marker))
}

fn parts(parser: &mut CodeParser, text: &str, lines: &[Span]) -> Vec<CommentPart> {
    let is_fence = |line: &Span| {
        let body = line.source_text(text).trim_start();
        body.starts_with("```") || body.starts_with("~~~")
    };
    // A line of code does not end with `.`, and a sentence that does would read on into the code
    // below it as a member: `Adds.` + `add(1, 2)` is `Adds.add(1, 2)`.
    let ends_sentence = |line: &Span| line.source_text(text).ends_with('.');
    let mut parts = Vec::new();
    let mut prose: Vec<Span> = Vec::new();
    let mut fenced = false;
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if line.is_empty() {
            flush(&mut parts, &mut prose, CommentKind::Prose, text);
            index += 1;
            continue;
        }
        // A Markdown fence marks an example: what it holds stays prose.
        let fence = is_fence(&line);
        fenced ^= fence;
        if !fence && !fenced && !ends_sentence(&line) {
            let limit = lines[index..]
                .iter()
                .position(|line| is_fence(line) || ends_sentence(line))
                .map_or(lines.len(), |offset| index + offset);
            if let Some(end) = code_run(parser, text, &lines[index..limit]).map(|last| index + last)
            {
                flush(&mut parts, &mut prose, CommentKind::Prose, text);
                let mut code: Vec<Span> = lines[index..=end]
                    .iter()
                    .copied()
                    .filter(|line| !line.is_empty())
                    .collect();
                flush(&mut parts, &mut code, CommentKind::Code, text);
                index = end + 1;
                continue;
            }
        }
        prose.push(line);
        index += 1;
    }
    flush(&mut parts, &mut prose, CommentKind::Prose, text);
    parts
}

fn flush(parts: &mut Vec<CommentPart>, lines: &mut Vec<Span>, kind: CommentKind, text: &str) {
    let (Some(first), Some(last)) = (lines.first(), lines.last()) else {
        return;
    };
    let body = first.source_text(text);
    let start = first.start + (body.len() - body.trim_start().len()) as u32;
    parts.push(CommentPart {
        kind,
        span: Span::new(start, last.end),
        lines: std::mem::take(lines),
    });
}

/// The last line of the longest run from the first line that parses as code. A parse that fails
/// on line `k` of the run fails the same way for every longer run, so the next try ends before `k`.
fn code_run(parser: &mut CodeParser, text: &str, lines: &[Span]) -> Option<usize> {
    if starts_as_words(lines[0].source_text(text)) {
        return None;
    }
    let mut source = String::new();
    let mut end = lines.len() - 1;
    loop {
        while end > 0 && lines[end].is_empty() {
            end -= 1;
        }
        source.clear();
        for (index, line) in lines[..=end].iter().enumerate() {
            if index > 0 {
                source.push('\n');
            }
            source.push_str(line.source_text(text));
        }
        let ends_with_comma = |at: usize| lines[at].source_text(text).ends_with(',');
        end = match parser.read(&source) {
            Reading::Code => return Some(end),
            _ if end == 0 => return None,
            Reading::Other => end - 1,
            Reading::Fails { line } => {
                // Only a run that ends with `,` is read as properties, so a shorter one past the
                // error may still be: `retries: 3,` + `cause: pgCause,` above `see the docs`.
                let properties = (!ends_with_comma(end))
                    .then(|| (line..end).rev().find(|&at| ends_with_comma(at)))
                    .flatten();
                match properties {
                    Some(at) => at,
                    None if line == 0 => return None,
                    None => (line - 1).min(end - 1),
                }
            }
        };
    }
}

enum Reading {
    Code,
    /// Parses, but as words that only look like code: a name, a label, `a - b`.
    Other,
    /// No reading parses; the furthest error is on this line of the text.
    Fails {
        line: usize,
    },
}

struct CodeParser {
    allocator: Allocator,
    source_type: SourceType,
    wrapped: String,
}

impl CodeParser {
    fn new(jsx: bool) -> CodeParser {
        CodeParser {
            allocator: Allocator::default(),
            source_type: SourceType::ts().with_jsx(jsx),
            wrapped: String::new(),
        }
    }

    /// Reads `source` as statements, as class members, as object properties (a run that ends
    /// with `,`) and as decorators (a run that starts with `@`).
    fn read(&mut self, source: &str) -> Reading {
        let readings: [(Wrap, bool); 4] = [
            (Wrap::Statements, true),
            (Wrap::Members, true),
            (Wrap::Properties, source.trim_end().ends_with(',')),
            (Wrap::Decorators, source.trim_start().starts_with('@')),
        ];
        let mut parsed = false;
        let mut furthest = 0;
        for (wrap, applies) in readings {
            if !applies {
                continue;
            }
            match self.parse(source, wrap) {
                Ok(true) => return Reading::Code,
                Ok(false) => parsed = true,
                Err(line) => furthest = furthest.max(line),
            }
        }
        if parsed {
            Reading::Other
        } else {
            Reading::Fails { line: furthest }
        }
    }

    /// `Ok(is code)` when the wrapped source parses, else `Err(line of the first error)`.
    fn parse(&mut self, source: &str, wrap: Wrap) -> Result<bool, usize> {
        let (before, after) = match wrap {
            Wrap::Statements => ("", ""),
            Wrap::Members => ("class _ {\n", "\n}"),
            Wrap::Properties => ("({\n", "\n})"),
            Wrap::Decorators => ("", "\nclass _ {}"),
        };
        self.wrapped.clear();
        self.wrapped.extend([before, source, after]);
        let wrapped = self.wrapped.as_str();
        self.allocator.reset();
        let parsed = Parser::new(&self.allocator, wrapped, self.source_type)
            .with_options(ParseOptions {
                allow_return_outside_function: true,
                preserve_parens: false,
                enable_ident_hashes: false,
                ..ParseOptions::default()
            })
            .parse();
        if let Some(offset) = parsed
            .diagnostics
            .iter()
            .flat_map(|diagnostic| {
                diagnostic
                    .labels
                    .iter()
                    .map(|label| label.offset() as usize)
            })
            .min()
            .or_else(|| parsed.diagnostics.first().map(|_| wrapped.len()))
        {
            let line = wrapped[..offset.min(wrapped.len())].matches('\n').count();
            let shift = before.matches('\n').count();
            return Err(line.saturating_sub(shift));
        }
        Ok(is_code(&parsed.program, wrapped, wrap))
    }
}

#[derive(Clone, Copy)]
enum Wrap {
    Statements,
    Members,
    Properties,
    Decorators,
}

fn is_code(program: &Program<'_>, source: &str, wrap: Wrap) -> bool {
    let body: Vec<&Statement> = program
        .body
        .iter()
        .filter(|statement| !matches!(statement, Statement::EmptyStatement(_)))
        .collect();
    match (wrap, body.as_slice()) {
        (Wrap::Statements, []) => false,
        (Wrap::Statements, statements) => statements
            .iter()
            .all(|statement| is_code_statement(statement, source)),
        (Wrap::Members, [Statement::ClassDeclaration(class)]) => {
            !class.body.body.is_empty()
                && class
                    .body
                    .body
                    .iter()
                    .all(|member| is_code_member(member, source))
        }
        (Wrap::Properties, [Statement::ExpressionStatement(statement)]) => {
            match statement.expression.without_parentheses() {
                Expression::ObjectExpression(object) => {
                    !object.properties.is_empty()
                        && object.properties.iter().all(|property| match property {
                            ObjectPropertyKind::ObjectProperty(property) => {
                                match &property.value {
                                    // `Note: important,` is words; `cause: pgCause,` and
                                    // `'x-api-key': key,` are a property.
                                    Expression::Identifier(value) => {
                                        is_code_name(&value.name)
                                            || !property.shorthand
                                                && property.key.static_name().is_none_or(|key| {
                                                    is_code_name(&key)
                                                        || !property.key.is_identifier()
                                                })
                                    }
                                    _ => true,
                                }
                            }
                            ObjectPropertyKind::SpreadProperty(_) => false,
                        })
                }
                _ => false,
            }
        }
        (Wrap::Decorators, [Statement::ClassDeclaration(class)]) => {
            !class.decorators.is_empty()
                && class
                    .decorators
                    .iter()
                    .all(|decorator| matches!(decorator.expression, Expression::CallExpression(_)))
        }
        _ => false,
    }
}

fn ends_with_semicolon(node: &impl GetSpan, source: &str) -> bool {
    node.span().source_text(source).ends_with(';')
}

/// A statement code would have and prose would not: a declaration, a call, an assignment, a
/// loop or `if` around such a statement. A bare name, a label (`Note: ..`), `return early` or
/// `break` without `;` read as words.
fn is_code_statement(statement: &Statement<'_>, source: &str) -> bool {
    let terminated = ends_with_semicolon(statement, source);
    match statement {
        Statement::ExpressionStatement(statement) => {
            is_code_expression(&statement.expression, terminated, source)
        }
        Statement::BlockStatement(block) => {
            !block.body.is_empty()
                && block.body.iter().all(|statement| {
                    matches!(statement, Statement::EmptyStatement(_))
                        || is_code_statement(statement, source)
                })
        }
        Statement::IfStatement(statement) => is_code_statement(&statement.consequent, source),
        Statement::ForStatement(statement) => is_code_statement(&statement.body, source),
        Statement::ForInStatement(statement) => is_code_statement(&statement.body, source),
        Statement::ForOfStatement(statement) => is_code_statement(&statement.body, source),
        Statement::WhileStatement(statement) => is_code_statement(&statement.body, source),
        Statement::DoWhileStatement(_)
        | Statement::SwitchStatement(_)
        | Statement::TryStatement(_) => true,
        Statement::ReturnStatement(statement) => {
            terminated
                || statement
                    .argument
                    .as_ref()
                    .is_some_and(|argument| !matches!(argument, Expression::Identifier(_)))
        }
        Statement::ThrowStatement(statement) => {
            terminated || !matches!(statement.argument, Expression::Identifier(_))
        }
        Statement::BreakStatement(_)
        | Statement::ContinueStatement(_)
        | Statement::DebuggerStatement(_) => terminated,
        _ => statement.is_declaration() || statement.is_module_declaration(),
    }
}

fn is_code_expression(expression: &Expression<'_>, terminated: bool, source: &str) -> bool {
    // `Retries (3)` and ``overwrites `species` `` are words; code writes `retry(3)`.
    let touches = |callee: &Expression<'_>| {
        !source[callee.span().end as usize..].starts_with(char::is_whitespace)
    };
    match expression.get_inner_expression() {
        Expression::CallExpression(call) => terminated || touches(&call.callee),
        Expression::TaggedTemplateExpression(tagged) => terminated || touches(&tagged.tag),
        Expression::ImportExpression(_)
        | Expression::UpdateExpression(_)
        | Expression::YieldExpression(_) => true,
        // `new users` is two words; `new User()` is code.
        Expression::NewExpression(new) => {
            new.span.source_text(source).ends_with(')') && (terminated || touches(&new.callee))
        }
        Expression::AwaitExpression(expression) => {
            terminated || is_code_expression(&expression.argument, false, source)
        }
        // `- item()` is a list item; `void run()` and `!ready()` are code. TypeScript deletes
        // only a member, so `delete it` is words.
        Expression::UnaryExpression(expression) => {
            let member = expression.argument.get_inner_expression();
            expression.operator.is_delete()
                && (member.is_member_expression()
                    || matches!(member, Expression::ChainExpression(_)))
                || terminated
                || !expression.operator.is_arithmetic()
                    && is_code_expression(&expression.argument, false, source)
        }
        Expression::ChainExpression(chain) => {
            matches!(
                chain.expression,
                oxc_ast::ast::ChainElement::CallExpression(_)
            )
        }
        Expression::AssignmentExpression(assignment) => {
            terminated
                || assignment.left.is_member_expression()
                || is_code_expression(&assignment.right, false, source)
        }
        Expression::SequenceExpression(sequence) => sequence
            .expressions
            .iter()
            .all(|expression| is_code_expression(expression, false, source)),
        Expression::Identifier(_)
        | Expression::ThisExpression(_)
        | Expression::Super(_)
        | Expression::NewTarget(_)
        | Expression::ImportMeta(_)
        | Expression::TemplateLiteral(_)
        | Expression::StringLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::RegExpLiteral(_)
        | Expression::StaticMemberExpression(_)
        | Expression::ComputedMemberExpression(_)
        | Expression::PrivateFieldExpression(_) => false,
        _ => terminated,
    }
}

fn is_code_member(member: &ClassElement<'_>, source: &str) -> bool {
    let terminated = ends_with_semicolon(member, source);
    match member {
        ClassElement::MethodDefinition(method) => method.value.body.is_some() || terminated,
        // `total = price * count` reads as a formula and `public API` as words; `private total = 0`
        // and `total = sum();` as a field.
        ClassElement::PropertyDefinition(property) => {
            let modified = property.accessibility.is_some()
                || property.readonly
                || property.r#static
                || property.declare
                || property.r#override;
            terminated
                || modified && (property.value.is_some() || property.type_annotation.is_some())
                || property
                    .decorators
                    .iter()
                    .any(|decorator| matches!(decorator.expression, Expression::CallExpression(_)))
                || property
                    .value
                    .as_ref()
                    .is_some_and(|value| is_code_expression(value, false, source))
        }
        ClassElement::AccessorProperty(property) => property.value.is_some() || terminated,
        ClassElement::StaticBlock(_) => true,
        ClassElement::TSIndexSignature(_) => terminated,
    }
}

/// A name only code would spell: `pgCause`, `API_KEY`, `v2`, `undefined`; not a plain word.
fn is_code_name(name: &str) -> bool {
    name == "undefined"
        || name
            .chars()
            .skip(1)
            .any(|ch| ch.is_uppercase() || ch.is_ascii_digit() || ch == '_' || ch == '$')
}

/// Words that can stand before another name on one line of code: `const total`, `export
/// default`, `private readonly`.
const LEADING_KEYWORDS: &[&str] = &[
    "abstract",
    "accessor",
    "as",
    "async",
    "await",
    "break",
    "case",
    "class",
    "const",
    "continue",
    "declare",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "for",
    "function",
    "get",
    "global",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "is",
    "keyof",
    "let",
    "module",
    "namespace",
    "new",
    "of",
    "override",
    "private",
    "protected",
    "public",
    "readonly",
    "return",
    "satisfies",
    "set",
    "static",
    "throw",
    "type",
    "typeof",
    "using",
    "var",
    "void",
    "while",
    "yield",
];

/// `The cache keeps ..`: a name, a space and another name start no line of code, so the parse
/// would fail on this line anyway.
fn starts_as_words(line: &str) -> bool {
    let is_name = |ch: char| ch.is_alphanumeric() || ch == '_' || ch == '$';
    let line = line.trim_start();
    let first_end = line.find(|ch: char| !is_name(ch)).unwrap_or(line.len());
    let (first, rest) = line.split_at(first_end);
    let after = rest.trim_start();
    let second_end = after.find(|ch: char| !is_name(ch)).unwrap_or(after.len());
    let second = &after[..second_end];
    first.starts_with(char::is_alphabetic)
        && rest.starts_with([' ', '\t'])
        && second.starts_with(char::is_alphabetic)
        && !LEADING_KEYWORDS.contains(&first)
        && !LEADING_KEYWORDS.contains(&second)
}
