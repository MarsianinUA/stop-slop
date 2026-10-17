use std::collections::HashSet;

use oxc_ast::AstKind;
use oxc_ast::ast::{
    BindingPattern, Expression, FormalParameter, TSLiteral, TSSignature, TSType, TSTypeName,
    TSTypeOperatorOperator, TSTypeReference, UnaryOperator, VariableDeclarationKind,
};
use oxc_semantic::{AstNode, SymbolId};

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

/// The global value `name` (`Proxy`, `Number`, `Map`): the file neither declares nor imports it.
pub fn is_global(file: &File<'_>, expression: &Expression<'_>, name: &str) -> bool {
    matches!(expression, Expression::Identifier(id)
        if id.name == name && file.semantic.is_reference_to_global_variable(id))
}

/// `ty` is one that `is` accepts, or a union of such types with `null` / `undefined`:
/// `boolean | null`.
pub fn is_nullable(ty: &TSType<'_>, is: impl Fn(&TSType<'_>) -> bool) -> bool {
    match ty {
        TSType::TSUnionType(union) => {
            union.types.iter().any(&is)
                && union.types.iter().all(|ty| {
                    is(ty) || matches!(ty, TSType::TSNullKeyword(_) | TSType::TSUndefinedKeyword(_))
                })
        }
        _ => is(ty),
    }
}

/// The node with the type assertions around it (`x!`, `x as T`, `<T>x`, `x satisfies T`): the
/// outermost one, or the node itself. A write through them (`(x as T).a = 1`) still writes `x`.
pub fn outer_assertion<'a>(file: &File<'a>, node: &AstNode<'a>) -> &'a AstNode<'a> {
    let nodes = file.semantic.nodes();
    let mut id = node.id();
    while matches!(
        nodes.parent_kind(id),
        AstKind::TSNonNullExpression(_)
            | AstKind::TSAsExpression(_)
            | AstKind::TSTypeAssertion(_)
            | AstKind::TSSatisfiesExpression(_)
    ) {
        id = nodes.parent_id(id);
    }
    nodes.get_node(id)
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
/// literal (not `null`), template or unary expression (not `void`), under `as`, `<T>`,
/// `satisfies` and `!`; or an unannotated `const` of the file that nothing writes, whose
/// initializer is known. `as unknown`, `as any` and `as object` hide the type.
pub fn is_known_value(file: &File<'_>, expression: &Expression<'_>) -> bool {
    is_known(file, expression, &mut HashSet::new())
}

/// `seen` holds the consts already followed, so `const a = b; const b = a;` ends.
fn is_known(file: &File<'_>, expression: &Expression<'_>, seen: &mut HashSet<SymbolId>) -> bool {
    let mut expression = expression;
    loop {
        if let Some((value, ty)) = expression_assertion(expression) {
            if matches!(
                ty,
                TSType::TSUnknownKeyword(_) | TSType::TSAnyKeyword(_) | TSType::TSObjectKeyword(_)
            ) {
                return false;
            }
            expression = value;
            continue;
        }
        expression = match expression {
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

/// A literal value: `'orders'`, `50`, `-1`, `60 * 1000`, `true`, a template without `${}`, also
/// under `as const`. Not `null`, a regex, an array or an object; `'a' as string` is not fixed.
/// `naming-case` wants CONSTANT_CASE for such a module `const`, `no-loose-constant` a home.
pub fn is_literal_value(expression: &Expression<'_>) -> bool {
    match expression {
        Expression::StringLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::BooleanLiteral(_) => true,
        Expression::TemplateLiteral(template) => template.expressions.is_empty(),
        Expression::UnaryExpression(unary) => {
            matches!(
                unary.operator,
                UnaryOperator::UnaryNegation | UnaryOperator::UnaryPlus
            ) && is_literal_value(&unary.argument)
        }
        Expression::BinaryExpression(binary) => {
            binary.operator.is_arithmetic()
                && is_literal_value(&binary.left)
                && is_literal_value(&binary.right)
        }
        _ => expression_assertion(expression)
            .is_some_and(|(value, ty)| is_const_assertion(ty) && is_literal_value(value)),
    }
}

/// `key` of the parameter's object is an option: the type is lib `Partial<..>`, or `key?:` in its
/// type literal or in the interface or type literal alias of this file it names, also through
/// `Readonly<..>`; `key: T | null` is data, whose `?? 0` handles `null`. Where the type does not
/// show the key (imported, not annotated, no such member), only an optional parameter
/// (`options?: X`, `options: X = {}`) reads as options.
pub fn is_option_key<'a>(file: &File<'a>, parameter: &'a FormalParameter<'a>, key: &str) -> bool {
    let ty = parameter
        .type_annotation
        .as_ref()
        .map(|annotation| &annotation.type_annotation);
    if ty.is_some_and(|ty| lib_type(file, ty, "Partial").is_some()) {
        return true;
    }
    ty.and_then(|ty| members(file, unwrapped_type(file, ty)))
        .and_then(|members| {
            members.iter().find_map(|member| match member {
                TSSignature::TSPropertySignature(property)
                    if property.key.static_name().is_some_and(|name| name == key) =>
                {
                    Some(property.optional)
                }
                _ => None,
            })
        })
        .unwrap_or(parameter.optional || parameter.initializer.is_some())
}

fn members<'a>(file: &File<'a>, ty: &'a TSType<'a>) -> Option<&'a [TSSignature<'a>]> {
    let TSType::TSTypeReference(reference) = ty else {
        return literal_members(ty);
    };
    let TSTypeName::IdentifierReference(id) = &reference.type_name else {
        return None;
    };
    let scoping = file.semantic.scoping();
    let symbol = scoping.get_reference(id.reference_id()).symbol_id()?;
    match file
        .semantic
        .nodes()
        .kind(scoping.symbol_declaration(symbol))
    {
        AstKind::TSInterfaceDeclaration(interface) => Some(&interface.body.body),
        AstKind::TSTypeAliasDeclaration(alias) => literal_members(&alias.type_annotation),
        _ => None,
    }
}

fn literal_members<'a>(ty: &'a TSType<'a>) -> Option<&'a [TSSignature<'a>]> {
    match ty {
        TSType::TSTypeLiteral(literal) => Some(&literal.members),
        _ => None,
    }
}

/// `boolean`, `true`, `false`, or a union of them with `null` / `undefined`.
pub fn is_boolean_type(ty: &TSType<'_>) -> bool {
    is_nullable(ty, |ty| match ty {
        TSType::TSBooleanKeyword(_) => true,
        TSType::TSLiteralType(literal) => matches!(literal.literal, TSLiteral::BooleanLiteral(_)),
        _ => false,
    })
}

/// A `boolean` annotation (`is_boolean_type`); without a type, only a `true` / `false` default
/// shows one.
pub fn is_boolean_parameter(parameter: &FormalParameter<'_>) -> bool {
    match &parameter.type_annotation {
        Some(ty) => is_boolean_type(&ty.type_annotation),
        None => matches!(
            parameter.initializer.as_deref(),
            Some(Expression::BooleanLiteral(_))
        ),
    }
}
