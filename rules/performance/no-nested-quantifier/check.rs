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
                let flags = literal.regex.flags.to_inline_string();
                LiteralParser::new(
                    &allocator,
                    literal.regex.pattern.text.as_str(),
                    Some(flags.as_str()),
                    at(literal.span.start + 1),
                )
                .parse()
            }
            kind => {
                let Some((pattern, flags)) = constructed(file, kind) else {
                    return;
                };
                ConstructorParser::new(
                    &allocator,
                    pattern.source_text(file.text),
                    flags.map(|flags| flags.source_text(file.text)),
                    at(pattern.start),
                )
                .parse()
            }
        };
        let Ok(pattern) = parsed else {
            return;
        };
        let mut found = Vec::new();
        nested(&pattern.body, &mut found);
        for quantifier in found {
            cx.report(quantifier.span).arg("part", quantifier);
        }
    }
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
    let literal = |argument: &Argument<'_>| match argument {
        Argument::StringLiteral(literal) => Some(literal.span),
        Argument::TemplateLiteral(template) if template.expressions.is_empty() => {
            Some(template.span)
        }
        _ => None,
    };
    let pattern = literal(arguments.first()?)?;
    Some((pattern, arguments.get(1).and_then(literal)))
}

/// Every quantifier of the form `splits` finds, inner ones too.
fn nested<'p>(disjunction: &'p Disjunction<'p>, found: &mut Vec<&'p Quantifier<'p>>) {
    for term in disjunction
        .body
        .iter()
        .flat_map(|alternative| &alternative.body)
    {
        let mut term = term;
        if let Term::Quantifier(quantifier) = term {
            if splits(quantifier) {
                found.push(quantifier);
            }
            term = &quantifier.body;
        }
        match term {
            Term::CapturingGroup(group) => nested(&group.body, found),
            Term::IgnoreGroup(group) => nested(&group.body, found),
            Term::LookAroundAssertion(assertion) => nested(&assertion.body, found),
            _ => {}
        }
    }
}

/// `(a+)+`, `(\w*)*`, `([a-z]+-?)+`: an unbounded quantifier over a group, one of whose
/// alternatives holds only quantified atoms, and nothing in it must match between two passes
/// but a single character repeated a varying number of times. A run of that character splits
/// between the passes in exponentially many ways, and the engine tries them all when what
/// follows the group fails.
fn splits(quantifier: &Quantifier<'_>) -> bool {
    let group = match &quantifier.body {
        Term::CapturingGroup(group) => &group.body,
        Term::IgnoreGroup(group) => &group.body,
        _ => return false,
    };
    quantifier.max.is_none() && group.body.iter().any(splits_alternative)
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
    let required: Vec<_> = terms.iter().filter(|term| term.min > 0).collect();
    match required.as_slice() {
        [] => terms.iter().any(|term| varies(term)),
        [term] => varies(term),
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
