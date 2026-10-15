use std::collections::HashSet;

use oxc_ast::AstKind;
use oxc_ast::ast::{
    BindingPattern, Expression, TSType, TSTypeName, TSTypeOperatorOperator, TSTypeReference,
    UnaryOperator, VariableDeclarationKind,
};
use oxc_semantic::SymbolId;

use crate::File;

/// `value as T` or `<T>value`: the value and the asserted type.
pub fn type_assertion<'a>(kind: AstKind<'a>) -> Option<(&'a Expression<'a>, &'a TSType<'a>)> {
    match kind {
        AstKind::TSAsExpression(e) => Some((&e.expression, &e.type_annotation)),
        AstKind::TSTypeAssertion(e) => Some((&e.expression, &e.type_annotation)),
        _ => None,
    }
}

/// The same for an expression: the next link of an assertion chain.
pub fn expression_assertion<'a>(
    expression: &'a Expression<'a>,
) -> Option<(&'a Expression<'a>, &'a TSType<'a>)> {
    match expression {
        Expression::TSAsExpression(e) => Some((&e.expression, &e.type_annotation)),
        Expression::TSTypeAssertion(e) => Some((&e.expression, &e.type_annotation)),
        _ => None,
    }
}

/// `as const` / `<const>`.
pub fn is_const_assertion(ty: &TSType<'_>) -> bool {
    ty.is_const_type_reference()
}

/// A reference to the TypeScript lib type `name` (`Record`, `Readonly`...): the file neither
/// declares nor imports that name, so the reference does not resolve to a symbol.
pub fn lib_type<'a>(
    file: &File<'_>,
    ty: &'a TSType<'a>,
    name: &str,
) -> Option<&'a TSTypeReference<'a>> {
    let TSType::TSTypeReference(reference) = ty else {
        return None;
    };
    let TSTypeName::IdentifierReference(id) = &reference.type_name else {
        return None;
    };
    (id.name == name && file.semantic.is_reference_to_global_variable(id)).then_some(reference)
}

/// `ty` without `readonly` and the lib `Readonly`, `Partial`, `Required`, `NonNullable`.
/// Parentheses are already gone: the engine parses without `preserve_parens`.
pub fn unwrapped_type<'a>(file: &File<'_>, ty: &'a TSType<'a>) -> &'a TSType<'a> {
    let mut ty = ty;
    loop {
        ty = match ty {
            TSType::TSTypeOperatorType(operator)
                if operator.operator == TSTypeOperatorOperator::Readonly =>
            {
                &operator.type_annotation
            }
            _ => {
                let Some(inner) = ["Readonly", "Partial", "Required", "NonNullable"]
                    .into_iter()
                    .find_map(|name| lib_type(file, ty, name))
                    .and_then(|wrapper| wrapper.type_arguments.as_ref()?.params.first())
                else {
                    return ty;
                };
                inner
            }
        }
    }
}

/// A key type that admits any key: `string`, `number`, `symbol`, `any`, `keyof any`, lib
/// `PropertyKey`, a template with a `string` or `number` slot (`` `id-${string}` ``),
/// `string & {}`, or a union with one.
pub fn is_open_key(file: &File<'_>, ty: &TSType<'_>) -> bool {
    match ty {
        TSType::TSStringKeyword(_)
        | TSType::TSNumberKeyword(_)
        | TSType::TSSymbolKeyword(_)
        | TSType::TSAnyKeyword(_) => true,
        TSType::TSTypeOperatorType(operator) => {
            operator.operator == TSTypeOperatorOperator::Keyof
                && matches!(operator.type_annotation, TSType::TSAnyKeyword(_))
        }
        TSType::TSTemplateLiteralType(template) => template.types.iter().any(|slot| {
            matches!(
                slot,
                TSType::TSStringKeyword(_) | TSType::TSNumberKeyword(_)
            )
        }),
        // `string & {}` keeps editor suggestions for literal members and still takes any string.
        TSType::TSIntersectionType(intersection) => {
            intersection
                .types
                .iter()
                .any(|member| matches!(member, TSType::TSStringKeyword(_)))
                && intersection.types.iter().all(|member| match member {
                    TSType::TSStringKeyword(_) => true,
                    TSType::TSTypeLiteral(literal) => literal.members.is_empty(),
                    _ => false,
                })
        }
        TSType::TSUnionType(union) => union.types.iter().any(|member| is_open_key(file, member)),
        _ => lib_type(file, ty, "PropertyKey").is_some(),
    }
}

/// The value's type is known from its own syntax: an object, array, function, class, `new`,
/// literal (not `null`), template or unary expression (not `void`), under `as`, `<T>`, `satisfies` and `!`;
/// or an unannotated `const` of the file that nothing writes, whose initializer is known.
pub fn is_known_value(file: &File<'_>, expression: &Expression<'_>) -> bool {
    is_known(file, expression, &mut HashSet::new())
}

/// `seen` holds the consts already followed, so `const a = b; const b = a;` ends.
fn is_known(file: &File<'_>, expression: &Expression<'_>, seen: &mut HashSet<SymbolId>) -> bool {
    let mut expression = expression;
    loop {
        expression = match expression {
            Expression::TSAsExpression(e) => &e.expression,
            Expression::TSTypeAssertion(e) => &e.expression,
            Expression::TSSatisfiesExpression(e) => &e.expression,
            Expression::TSNonNullExpression(e) => &e.expression,
            _ => break,
        };
    }
    match expression {
        Expression::ObjectExpression(_)
        | Expression::ArrayExpression(_)
        | Expression::ArrowFunctionExpression(_)
        | Expression::FunctionExpression(_)
        | Expression::ClassExpression(_)
        | Expression::NewExpression(_)
        | Expression::StringLiteral(_)
        | Expression::TemplateLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::RegExpLiteral(_) => true,
        // `void 0` is `undefined`, which is not known, like `null`.
        Expression::UnaryExpression(unary) => unary.operator != UnaryOperator::Void,
        Expression::Identifier(id) => {
            let scoping = file.semantic.scoping();
            let Some(symbol) = scoping.get_reference(id.reference_id()).symbol_id() else {
                return false;
            };
            let nodes = file.semantic.nodes();
            let declaration = scoping.symbol_declaration(symbol);
            let (AstKind::VariableDeclarator(declarator), AstKind::VariableDeclaration(statement)) =
                (nodes.kind(declaration), nodes.parent_kind(declaration))
            else {
                return false;
            };
            statement.kind == VariableDeclarationKind::Const
                && matches!(declarator.id, BindingPattern::BindingIdentifier(_))
                && declarator.type_annotation.is_none()
                && seen.insert(symbol)
                && !scoping
                    .get_resolved_references(symbol)
                    .any(|r| r.is_write())
                && declarator
                    .init
                    .as_ref()
                    .is_some_and(|init| is_known(file, init, seen))
        }
        _ => false,
    }
}
