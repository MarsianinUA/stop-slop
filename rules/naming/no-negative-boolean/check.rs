use std::collections::BTreeMap;

use kit::oxc::ast::ast::{Expression, TSType, TSTypeAnnotation, TSTypeName};
use kit::oxc::semantic::{SymbolFlags, SymbolId};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// A word and its opposite (`Active` -> `Inactive`); the word is looked up without case.
    antonyms: BTreeMap<String, String>,
}

/// A prefix and the words that negate it right after it. `is` + `No` is mostly a term
/// (`isNoScriptError` for Redis `NOSCRIPT`).
const NEGATIONS: &[(&str, &[&str])] = &[
    ("is", &["not", "non"]),
    ("has", &["no", "not"]),
    ("can", &["not"]),
    ("should", &["not"]),
];

impl Rule for Check {
    const NODES: &'static [AstType] = PRIVATE_MEMBER_NODES;

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let Some((name, span)) = private_member(file, node) else {
            return;
        };
        let return_type = match node.kind() {
            AstKind::MethodDefinition(method) => method.value.return_type.as_deref(),
            _ => None,
        };
        if is_negative(name) && !guards_type_named(name, return_type) {
            // With its `#`, as the reader sees it.
            cx.report(span).arg("name", span.source_text(file.text));
        }
    }

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let scoping = file.semantic.scoping();
        let nodes = file.semantic.nodes();
        for symbol in scoping.symbol_ids() {
            let name = scoping.symbol_name(symbol);
            // A class `IsNotFoundError` or a type is not a boolean, and a destructured name
            // (`const { isNotValid } = props`) comes from its key.
            let (is_value, return_type) = match nodes.kind(scoping.symbol_declaration(symbol)) {
                AstKind::VariableDeclarator(declarator) => (
                    declarator.id.is_binding_identifier(),
                    match declarator
                        .init
                        .as_ref()
                        .map(Expression::get_inner_expression)
                    {
                        Some(Expression::ArrowFunctionExpression(arrow)) => {
                            arrow.return_type.as_deref()
                        }
                        Some(Expression::FunctionExpression(function)) => {
                            function.return_type.as_deref()
                        }
                        _ => None,
                    },
                ),
                AstKind::FormalParameter(parameter) => {
                    (parameter.pattern.is_binding_identifier(), None)
                }
                AstKind::FormalParameterRest(parameter) => {
                    (parameter.rest.argument.is_binding_identifier(), None)
                }
                AstKind::Function(function) => (true, function.return_type.as_deref()),
                _ => (false, None),
            };
            if !is_value
                || !is_negative(name)
                || guards_type_named(name, return_type)
                || !is_own_name(file, symbol)
            {
                continue;
            }
            let edits = self
                .positive(name)
                .filter(|positive| !self.has_clashing_neighbour(file, symbol, positive))
                .and_then(|positive| rename(file, symbol, &positive));
            cx.report(scoping.symbol_span(symbol))
                .arg("name", name)
                .fix(edits.unwrap_or_default());
        }
    }
}

impl Check {
    /// `isNotActive` -> `isInactive`, `IS_NOT_VISIBLE` -> `IS_HIDDEN`. Only `is` + `Not` + a word
    /// from `antonyms`, in a name of one case: `is_not_active` waits for `naming-case`, `hasNoRows`
    /// needs the positive flag negated where it is read, not a rename, and a longer name may hold
    /// a term (`isNotFoundError` is not `isMissingError`).
    fn positive(&self, name: &str) -> Option<String> {
        let constant = Case::Constant.matches(name);
        if !constant && !Case::Camel.matches(name) && !Case::Pascal.matches(name) {
            return None;
        }
        let words = words(name);
        let [is, not, word] = words.as_slice() else {
            return None;
        };
        if !is.eq_ignore_ascii_case("is") || !not.eq_ignore_ascii_case("not") {
            return None;
        }
        let antonym = self
            .antonyms
            .iter()
            .find_map(|(key, antonym)| key.eq_ignore_ascii_case(word).then_some(antonym))?;
        let antonym = if word.chars().any(char::is_lowercase) {
            antonym.clone()
        } else {
            antonym.to_uppercase()
        };
        // `is` or `IS_`, as written.
        let head = &name[..is.len() + usize::from(constant)];
        Some(format!("{head}{antonym}"))
    }

    /// A value in a related scope that may take the name `positive` too in the same pass: one
    /// named `positive` without its first word, ignoring `_`, `-`, `$` and case, which
    /// `boolean-prefix` may rename (`disabled` while `isNotEnabled` becomes `isDisabled`), or
    /// another negative name with the same opposite (`isNotSame` next to `isNotEqual`, both
    /// `isDifferent`).
    fn has_clashing_neighbour(&self, file: &File<'_>, symbol: SymbolId, positive: &str) -> bool {
        let scoping = file.semantic.scoping();
        let name = scoping.symbol_name(symbol);
        let unprefixed = words(positive)[1..].concat();
        related_symbols(file, symbol).any(|other| {
            let other_name = scoping.symbol_name(other);
            other_name != name
                && scoping.symbol_flags(other).intersects(SymbolFlags::Value)
                && (is_similar_name(other_name, &unprefixed)
                    || self
                        .positive(other_name)
                        .is_some_and(|other| is_similar_name(&other, positive)))
        })
    }
}

fn is_negative(name: &str) -> bool {
    let words = words(name);
    let [prefix, negation, _, ..] = words.as_slice() else {
        return false;
    };
    NEGATIONS.iter().any(|(word, negations)| {
        prefix.eq_ignore_ascii_case(word)
            && negations
                .iter()
                .any(|negative| negation.eq_ignore_ascii_case(negative))
    })
}

/// A type guard named `is` + the type it checks asks about that type, not a negation:
/// `isNotFoundError(error): error is NotFoundError`.
fn guards_type_named(name: &str, return_type: Option<&TSTypeAnnotation<'_>>) -> bool {
    let Some(TSType::TSTypePredicate(predicate)) = return_type.map(|ty| &ty.type_annotation) else {
        return false;
    };
    let Some(TSType::TSTypeReference(reference)) = predicate
        .type_annotation
        .as_ref()
        .map(|ty| &ty.type_annotation)
    else {
        return false;
    };
    let type_name = match &reference.type_name {
        TSTypeName::IdentifierReference(id) => id.name.as_str(),
        TSTypeName::QualifiedName(name) => name.right.name.as_str(),
        _ => return false,
    };
    name.strip_prefix("is") == Some(type_name)
}
