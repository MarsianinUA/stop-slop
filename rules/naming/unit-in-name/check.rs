use std::collections::HashSet;

use kit::oxc::ast::ast::{
    BinaryOperator, Expression, TSLiteral, TSType, TSTypeAnnotation, UnaryOperator,
};
use kit::oxc::semantic::SymbolId;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// Words a numeric name may end with, without case.
    quantities: Vec<String>,
    /// Words that name the unit, without case.
    units: Vec<String>,
}

impl Rule for Check {
    const NODES: &'static [AstType] = &[
        AstType::PropertyDefinition,
        AstType::AccessorProperty,
        AstType::FormalParameter,
    ];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let Some((name, span)) = private_member(file, node) else {
            return;
        };
        let Some(quantity) = self.missing_unit(name) else {
            return;
        };
        let (ty, value) = match node.kind() {
            AstKind::PropertyDefinition(property) => {
                (property.type_annotation.as_deref(), property.value.as_ref())
            }
            AstKind::AccessorProperty(accessor) => {
                (accessor.type_annotation.as_deref(), accessor.value.as_ref())
            }
            AstKind::FormalParameter(parameter) => (
                parameter.type_annotation.as_deref(),
                parameter.initializer.as_deref(),
            ),
            _ => return,
        };
        if is_number_value(file, ty, value, &mut HashSet::new()) {
            // With its `#`, as the reader sees it.
            cx.report(span)
                .arg("name", span.source_text(file.text))
                .arg("quantity", quantity);
        }
    }

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let scoping = file.semantic.scoping();
        for symbol in scoping.symbol_ids() {
            let name = scoping.symbol_name(symbol);
            let Some(quantity) = self.missing_unit(name) else {
                continue;
            };
            if is_own_name(file, symbol) && is_number_symbol(file, symbol, &mut HashSet::new()) {
                cx.report(scoping.symbol_span(symbol))
                    .arg("name", name)
                    .arg("quantity", quantity);
            }
        }
    }
}

impl Check {
    /// The quantity the name ends with when no word of it is a unit: `cacheTtl` -> `ttl`;
    /// `REALTIME_TTL_MULTIPLIER` is a multiplier.
    fn missing_unit(&self, name: &str) -> Option<&str> {
        let words = words(name);
        let last = words.last()?;
        let quantity = self
            .quantities
            .iter()
            .find(|quantity| quantity.eq_ignore_ascii_case(last))?;
        let has_unit = words.iter().any(|word| {
            self.units
                .iter()
                .any(|unit| unit.eq_ignore_ascii_case(word))
        });
        (!has_unit).then_some(quantity.as_str())
    }
}

/// A variable or a parameter the file declares directly: a destructured name comes from its
/// key. `seen` ends a cycle of variables (`const a = b; const b = a;`).
fn is_number_symbol(file: &File<'_>, symbol: SymbolId, seen: &mut HashSet<SymbolId>) -> bool {
    let declaration = file.semantic.scoping().symbol_declaration(symbol);
    let (ty, value) = match file.semantic.nodes().kind(declaration) {
        AstKind::VariableDeclarator(declarator) if declarator.id.is_binding_identifier() => (
            declarator.type_annotation.as_deref(),
            declarator.init.as_ref(),
        ),
        AstKind::FormalParameter(parameter) if parameter.pattern.is_binding_identifier() => (
            parameter.type_annotation.as_deref(),
            parameter.initializer.as_deref(),
        ),
        _ => return false,
    };
    is_number_value(file, ty, value, seen)
}

fn is_number_value(
    file: &File<'_>,
    ty: Option<&TSTypeAnnotation<'_>>,
    value: Option<&Expression<'_>>,
    seen: &mut HashSet<SymbolId>,
) -> bool {
    match ty {
        Some(ty) => is_number_type(&ty.type_annotation),
        None => value.is_some_and(|value| is_number(file, value, seen)),
    }
}

/// `number`, `bigint`, a numeric literal, or a union of them with `null` / `undefined`.
fn is_number_type(ty: &TSType<'_>) -> bool {
    is_nullable(ty, |ty| match ty {
        TSType::TSNumberKeyword(_) | TSType::TSBigIntKeyword(_) => true,
        TSType::TSLiteralType(literal) => matches!(
            literal.literal,
            TSLiteral::NumericLiteral(_) | TSLiteral::BigIntLiteral(_)
        ),
        _ => false,
    })
}

/// Only what the syntax shows: an operator other than `+` gives a number whatever it gets
/// (`deadline - now`); `setTimeout(..)` returns a handle, and a call other than the global
/// `Number(..)`, `parseInt(..)`, `parseFloat(..)`, `Math.*(..)` and `Date.now()` shows nothing.
fn is_number(file: &File<'_>, expression: &Expression<'_>, seen: &mut HashSet<SymbolId>) -> bool {
    match expression {
        Expression::TSNonNullExpression(inner) => is_number(file, &inner.expression, seen),
        Expression::TSSatisfiesExpression(inner) => is_number(file, &inner.expression, seen),
        // `5000 as const` keeps the literal.
        Expression::TSAsExpression(assertion) if is_const_assertion(&assertion.type_annotation) => {
            is_number(file, &assertion.expression, seen)
        }
        Expression::TSAsExpression(assertion) => is_number_type(&assertion.type_annotation),
        Expression::TSTypeAssertion(assertion) => is_number_type(&assertion.type_annotation),
        Expression::NumericLiteral(_) | Expression::BigIntLiteral(_) => true,
        Expression::UnaryExpression(unary) => matches!(
            unary.operator,
            UnaryOperator::UnaryPlus | UnaryOperator::UnaryNegation | UnaryOperator::BitwiseNot
        ),
        // `+` joins strings too.
        Expression::BinaryExpression(binary) if binary.operator == BinaryOperator::Addition => {
            is_number(file, &binary.left, seen) && is_number(file, &binary.right, seen)
        }
        Expression::BinaryExpression(binary) => {
            binary.operator.is_arithmetic() || binary.operator.is_bitwise()
        }
        Expression::CallExpression(call) => {
            matches!(
                global_member(file, &call.callee),
                Some(("Math", _) | ("Date", "now"))
            ) || ["Number", "parseInt", "parseFloat"]
                .iter()
                .any(|name| is_global(file, &call.callee, name))
        }
        Expression::Identifier(id) => file
            .semantic
            .scoping()
            .get_reference(id.reference_id())
            .symbol_id()
            .is_some_and(|symbol| {
                // Only the variables on the way here: `a + a` reads one `a` twice.
                if !seen.insert(symbol) {
                    return false;
                }
                let is_number = is_number_symbol(file, symbol, seen);
                seen.remove(&symbol);
                is_number
            }),
        _ => false,
    }
}
