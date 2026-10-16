use std::collections::HashSet;

use kit::oxc::ast::ast::{
    BindingPattern, Expression, FormalParameter, MethodDefinitionKind, TSLiteral, TSType,
    TSTypeAnnotation, UnaryOperator,
};
use kit::oxc::semantic::{SymbolFlags, SymbolId};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// One-word states that read right after `is` (`ready` -> `isReady`), without case. Any other
    /// name may change its meaning with `is` (`advanced`), so it gets no fix.
    states: Vec<String>,
}

const PREFIXES: &[&str] = &["is", "has", "can", "should"];

#[derive(PartialEq)]
enum Boolean {
    Value,
    /// A type guard `x is T`: a function, so its name stays for the agent to choose.
    Guard,
}

impl Rule for Check {
    const NODES: &'static [AstType] = PRIVATE_MEMBER_NODES;

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let Some((name, span)) = private_member(file, node) else {
            return;
        };
        if needs_prefix(name) && is_boolean_member(file, node) {
            // With its `#`, as the reader sees it.
            let shown = span.source_text(file.text);
            cx.report(span)
                .arg("name", shown)
                .arg("suggestion", shown.replacen(name, &with_is(name), 1));
        }
    }

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let scoping = file.semantic.scoping();
        for symbol in scoping.symbol_ids() {
            let name = scoping.symbol_name(symbol);
            if !needs_prefix(name) || !is_own_name(file, symbol) {
                continue;
            }
            let Some(boolean) = boolean(file, symbol, &mut HashSet::new()) else {
                continue;
            };
            let suggestion = with_is(name);
            let edits = if boolean == Boolean::Value
                && self.is_state(name)
                && !has_clashing_neighbour(file, symbol)
            {
                rename(file, symbol, &suggestion)
            } else {
                None
            };
            cx.report(scoping.symbol_span(symbol))
                .arg("name", name)
                .arg("suggestion", &suggestion)
                .fix(edits.unwrap_or_default());
        }
    }
}

impl Check {
    fn is_state(&self, name: &str) -> bool {
        let words = words(name);
        let [word] = words.as_slice() else {
            return false;
        };
        self.states
            .iter()
            .any(|state| state.eq_ignore_ascii_case(word))
    }
}

/// The first word is not a prefix: `_isDraft` and `IS_STRICT` read as questions, `island` does
/// not.
fn needs_prefix(name: &str) -> bool {
    words(name).first().is_some_and(|first| {
        !PREFIXES
            .iter()
            .any(|prefix| first.eq_ignore_ascii_case(prefix))
    })
}

/// `ready` -> `isReady`, `CACHED` -> `IS_CACHED`; leading `_` stay.
fn with_is(name: &str) -> String {
    let bare = name.trim_start_matches('_');
    let underscores = &name[..name.len() - bare.len()];
    if Case::Constant.matches(bare) {
        return format!("{underscores}IS_{bare}");
    }
    let mut chars = bare.chars();
    let first: String = chars
        .next()
        .into_iter()
        .flat_map(char::to_uppercase)
        .collect();
    format!("{underscores}is{first}{}", chars.as_str())
}

/// What the syntax shows the symbol to be. Only a name the file declares directly counts: a
/// destructured name (`{ dryRun = false }`) comes from its key. `seen` ends a cycle of
/// variables (`const a = b; const b = a;`).
fn boolean(file: &File<'_>, symbol: SymbolId, seen: &mut HashSet<SymbolId>) -> Option<Boolean> {
    let declaration = file.semantic.scoping().symbol_declaration(symbol);
    let is_this = |pattern: &BindingPattern<'_>| {
        matches!(pattern, BindingPattern::BindingIdentifier(id)
            if id.symbol_id.get() == Some(symbol))
    };
    let value = |is_boolean: bool| is_boolean.then_some(Boolean::Value);
    match file.semantic.nodes().kind(declaration) {
        AstKind::VariableDeclarator(declarator) if is_this(&declarator.id) => {
            match (&declarator.type_annotation, &declarator.init) {
                (Some(ty), _) => value(is_boolean_type(&ty.type_annotation)),
                (None, Some(init)) => match init.get_inner_expression() {
                    Expression::ArrowFunctionExpression(arrow) => {
                        guard(arrow.return_type.as_deref())
                    }
                    Expression::FunctionExpression(function) => {
                        guard(function.return_type.as_deref())
                    }
                    _ => value(is_boolean(file, init, seen)),
                },
                (None, None) => None,
            }
        }
        AstKind::FormalParameter(parameter) if is_this(&parameter.pattern) => {
            value(is_boolean_parameter(parameter))
        }
        AstKind::Function(function) => guard(function.return_type.as_deref()),
        _ => None,
    }
}

/// `x is T`; `asserts x is T` returns nothing.
fn guard(return_type: Option<&TSTypeAnnotation<'_>>) -> Option<Boolean> {
    let is_guard = return_type.is_some_and(|ty| {
        matches!(&ty.type_annotation, TSType::TSTypePredicate(predicate) if !predicate.asserts)
    });
    is_guard.then_some(Boolean::Guard)
}

fn is_boolean_member(file: &File<'_>, node: &AstNode<'_>) -> bool {
    let field = |ty: Option<&TSTypeAnnotation<'_>>, value: Option<&Expression<'_>>| match ty {
        Some(ty) => is_boolean_type(&ty.type_annotation),
        None => value.is_some_and(|value| is_boolean(file, value, &mut HashSet::new())),
    };
    match node.kind() {
        AstKind::PropertyDefinition(property) => {
            field(property.type_annotation.as_deref(), property.value.as_ref())
        }
        AstKind::AccessorProperty(accessor) => {
            field(accessor.type_annotation.as_deref(), accessor.value.as_ref())
        }
        AstKind::FormalParameter(parameter) => is_boolean_parameter(parameter),
        AstKind::MethodDefinition(method) => {
            let return_type = method.value.return_type.as_deref();
            match method.kind {
                MethodDefinitionKind::Get => {
                    return_type.is_some_and(|ty| is_boolean_type(&ty.type_annotation))
                }
                MethodDefinitionKind::Method => guard(return_type).is_some(),
                _ => false,
            }
        }
        _ => false,
    }
}

/// Without a type, only a `true` / `false` default shows one.
fn is_boolean_parameter(parameter: &FormalParameter<'_>) -> bool {
    match &parameter.type_annotation {
        Some(ty) => is_boolean_type(&ty.type_annotation),
        None => matches!(
            parameter.initializer.as_deref(),
            Some(Expression::BooleanLiteral(_))
        ),
    }
}

/// `boolean`, `true`, `false`, or a union of them with `null` / `undefined`.
fn is_boolean_type(ty: &TSType<'_>) -> bool {
    is_nullable(ty, |ty| match ty {
        TSType::TSBooleanKeyword(_) => true,
        TSType::TSLiteralType(literal) => matches!(literal.literal, TSLiteral::BooleanLiteral(_)),
        _ => false,
    })
}

/// Only what the syntax shows: a call other than `Boolean(x)` and a few global `is*` checks
/// (`check()`, `.includes()`) shows nothing.
fn is_boolean(file: &File<'_>, expression: &Expression<'_>, seen: &mut HashSet<SymbolId>) -> bool {
    match expression {
        Expression::TSNonNullExpression(inner) => is_boolean(file, &inner.expression, seen),
        Expression::TSSatisfiesExpression(inner) => is_boolean(file, &inner.expression, seen),
        Expression::TSAsExpression(assertion) => is_boolean_type(&assertion.type_annotation),
        Expression::TSTypeAssertion(assertion) => is_boolean_type(&assertion.type_annotation),
        Expression::BooleanLiteral(_) => true,
        Expression::UnaryExpression(unary) => unary.operator == UnaryOperator::LogicalNot,
        Expression::BinaryExpression(binary) => {
            binary.operator.is_equality()
                || binary.operator.is_compare()
                || binary.operator.is_relational()
        }
        Expression::LogicalExpression(logical) => {
            is_boolean(file, &logical.left, seen) && is_boolean(file, &logical.right, seen)
        }
        Expression::ConditionalExpression(conditional) => {
            is_boolean(file, &conditional.consequent, seen)
                && is_boolean(file, &conditional.alternate, seen)
        }
        Expression::CallExpression(call) => match &call.callee {
            Expression::StaticMemberExpression(member) => {
                let method = member.property.name.as_str();
                (is_global(file, &member.object, "Array") && method == "isArray")
                    || (is_global(file, &member.object, "Number")
                        && matches!(method, "isNaN" | "isFinite" | "isInteger" | "isSafeInteger"))
            }
            callee => is_global(file, callee, "Boolean"),
        },
        Expression::Identifier(id) => file
            .semantic
            .scoping()
            .get_reference(id.reference_id())
            .symbol_id()
            .is_some_and(|symbol| {
                // Only the variables on the way here: `one && two` may both read one `flag`.
                if !seen.insert(symbol) {
                    return false;
                }
                let is_value = boolean(file, symbol, seen) == Some(Boolean::Value);
                seen.remove(&symbol);
                is_value
            }),
        _ => false,
    }
}

/// A value in a related scope that may take this fix's name too in the same pass: one named
/// `isNot<Word>`, written in CONSTANT_CASE exactly when this name is, which `no-negative-boolean`
/// may rename to `is<Antonym>`; or one whose name differs from this one only in `_`, `-`, `$`
/// and case (`Ready` next to `ready`, both `isReady`).
fn has_clashing_neighbour(file: &File<'_>, symbol: SymbolId) -> bool {
    let scoping = file.semantic.scoping();
    let name = scoping.symbol_name(symbol);
    let is_constant = |name: &str| Case::Constant.matches(name.trim_start_matches('_'));
    let constant = is_constant(name);
    related_symbols(file, symbol).any(|other| {
        let other_name = scoping.symbol_name(other);
        other_name != name
            && scoping.symbol_flags(other).intersects(SymbolFlags::Value)
            && (is_similar_name(other_name, name)
                || matches!(words(other_name).as_slice(), [is, not, _]
                    if is.eq_ignore_ascii_case("is") && not.eq_ignore_ascii_case("not"))
                    && is_constant(other_name) == constant)
    })
}
