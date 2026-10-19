use kit::oxc::ast::ast::Argument;
use kit::oxc::regular_expression::ast::{Alternative, Disjunction, Quantifier, Term};
use kit::oxc::regular_expression::{ConstructorParser, LiteralParser, Options};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[
        AstType::RegExpLiteral,
        AstType::NewExpression,
        AstType::CallExpression,
    ];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let allocator = Allocator::default();
        // The engine parses without `parse_regular_expression`: there a bad pattern would turn
        // the whole file into a `parse-error`. Here it only leaves the rule silent.
        let parsed = match node.kind() {
            AstKind::RegExpLiteral(literal) => {
                let pattern = literal.regex.pattern.text.as_str();
                if !has_quantified_group(pattern) {
                    return;
                }
                let flags = literal.regex.flags.to_inline_string();
                LiteralParser::new(
                    &allocator,
                    pattern,
                    Some(flags.as_str()),
                    at(literal.span.start + 1),
                )
                .parse()
            }
            kind => {
                let Some((pattern, flags)) = constructed(file, kind) else {
                    return;
                };
                let text = pattern.source_text(file.text);
                if !has_quantified_group(text) {
                    return;
                }
                ConstructorParser::new(
                    &allocator,
                    text,
                    flags.map(|flags| flags.source_text(file.text)),
                    at(pattern.start),
                )
                .parse()
            }
        };
        let Ok(pattern) = parsed else {
            return;
        };
        nested(&pattern.body, cx);
    }
}

/// A group with a quantifier right after it, which a finding needs: most patterns have none and
/// skip the parse. A false match only costs the parse.
fn has_quantified_group(pattern: &str) -> bool {
    pattern
        .as_bytes()
        .windows(2)
        .any(|pair| pair[0] == b')' && matches!(pair[1], b'+' | b'*' | b'{'))
}

/// Spans of the pattern come out in the file's offsets.
fn at(pattern_span_offset: u32) -> Options {
    Options {
        pattern_span_offset,
        ..Options::default()
    }
}

/// The literals of the pattern and the flags given to the global `RegExp`, with or without
/// `new`: `'(x+)+'` and `'u'` of `new RegExp('(x+)+', 'u')`. A string or a template without
/// `${}`; a pattern built at run time has no text to read.
fn constructed(file: &File<'_>, kind: AstKind<'_>) -> Option<(Span, Option<Span>)> {
    let (callee, arguments) = match kind {
        AstKind::NewExpression(new) => (&new.callee, &new.arguments),
        AstKind::CallExpression(call) => (&call.callee, &call.arguments),
        _ => return None,
    };
    if !is_global(file, callee.get_inner_expression(), "RegExp") {
        return None;
    }
    let literal = |argument: &Argument<'_>| {
        argument
            .as_expression()
            .filter(|argument| static_text(argument).is_some())
            .map(GetSpan::span)
    };
    let pattern = literal(arguments.first()?)?;
    Some((pattern, arguments.get(1).and_then(literal)))
}

/// Reports every quantifier of the form `splits` finds, inner ones too.
fn nested(disjunction: &Disjunction<'_>, cx: &mut Cx<'_, '_>) {
    for term in disjunction
        .body
        .iter()
        .flat_map(|alternative| &alternative.body)
    {
        let term = match term {
            Term::Quantifier(quantifier) => {
                if splits(quantifier) {
                    cx.report(quantifier.span).arg("part", quantifier);
                }
                &quantifier.body
            }
            term => term,
        };
        let body = match term {
            Term::LookAroundAssertion(assertion) => Some(&assertion.body),
            term => group_body(term),
        };
        if let Some(body) = body {
            nested(body, cx);
        }
    }
}

fn group_body<'b, 'p>(term: &'b Term<'p>) -> Option<&'b Disjunction<'p>> {
    match term {
        Term::CapturingGroup(group) => Some(&group.body),
        Term::IgnoreGroup(group) => Some(&group.body),
        _ => None,
    }
}

/// `(a+)+`, `(\w*)*`, `([a-z]+-?)+`: an unbounded quantifier over a group, one of whose
/// alternatives holds only quantified atoms, and nothing in it must match between two passes
/// but a single character repeated a varying number of times. A run of that character splits
/// between the passes in exponentially many ways, and the engine tries them all when what
/// follows the group fails.
fn splits(quantifier: &Quantifier<'_>) -> bool {
    quantifier.max.is_none()
        && group_body(&quantifier.body)
            .is_some_and(|group| group.body.iter().any(splits_alternative))
}

/// At most one term is required, and it varies; with none required, any one varies.
fn splits_alternative(alternative: &Alternative<'_>) -> bool {
    let mut terms = Vec::new();
    for term in &alternative.body {
        let Term::Quantifier(term) = term else {
            return false;
        };
        terms.push(term);
    }
    // A pass can take the character two different non-zero numbers of times: an empty pass
    // ends the loop.
    let varies = |term: &Quantifier<'_>| {
        is_character(&term.body) && term.max.is_none_or(|max| max > term.min.max(1))
    };
    let mut required = terms.iter().filter(|term| term.min > 0);
    match (required.next(), required.next()) {
        (None, _) => terms.iter().any(|term| varies(term)),
        (Some(term), None) => varies(term),
        _ => false,
    }
}

/// An atom that matches exactly one character: `a`, `.`, `\d`, `\p{L}`, `[a-z]`.
fn is_character(term: &Term<'_>) -> bool {
    match term {
        Term::Character(_) | Term::Dot(_) | Term::CharacterClassEscape(_) => true,
        Term::UnicodePropertyEscape(escape) => !escape.strings,
        // `[]` matches nothing.
        Term::CharacterClass(class) => !class.strings && (class.negative || !class.body.is_empty()),
        _ => false,
    }
}
