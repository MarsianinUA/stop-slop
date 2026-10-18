use std::collections::HashSet;

use oxc_ast::AstKind;
use oxc_ast::ast::{
    BindingPattern, Expression, FormalParameter, TSLiteral, TSSignature, TSType, TSTypeName,
    TSTypeOperatorOperator, TSTypeReference, UnaryOperator, VariableDeclarationKind,
};
use oxc_semantic::{AstNode, SymbolId};
use oxc_span::GetSpan;

use crate::{File, call_of, is_exported};

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

/// Every reference to the global value `name` (`process`, `console`) in the file: the
/// `IdentifierReference` nodes no declaration or import holds, `typeof name` in a type too.
pub fn global_references<'a>(
    file: &File<'a>,
    name: &str,
) -> impl Iterator<Item = &'a AstNode<'a>> + 'a {
    let scoping = file.semantic.scoping();
    let nodes = file.semantic.nodes();
    scoping
        .root_unresolved_references()
        .get(name)
        .into_iter()
        .flatten()
        .map(move |&reference| nodes.get_node(scoping.get_reference(reference).node_id()))
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

/// A value that a `const` or a `readonly` field types as its own literal, where `let` and a plain
/// field widen it: `'fast'` is `'fast'`, not `string`; so are `null`, `Kind.User`,
/// `cond ? 'a' : 'b'`, a name or member that may hold such a literal, and a call or tagged
/// template given such a value (`pick('fast')` of `pick<T>(value: T): T`), also under `await`.
/// An object, array, function, class, `new`, another call, regex, `a + b`, a template with
/// `${}`, `x as T` and `<T>x` keep their type.
pub fn narrows_to_literal(value: &Expression<'_>) -> bool {
    match value {
        Expression::CallExpression(call) => call
            .arguments
            .iter()
            .any(|argument| argument.as_expression().is_none_or(narrows_to_literal)),
        Expression::TaggedTemplateExpression(tagged) => {
            tagged.quasi.expressions.iter().any(narrows_to_literal)
        }
        Expression::AwaitExpression(awaited) => narrows_to_literal(&awaited.argument),
        Expression::TemplateLiteral(template) => template.expressions.is_empty(),
        _ => !matches!(
            value,
            Expression::ObjectExpression(_)
                | Expression::ArrayExpression(_)
                | Expression::FunctionExpression(_)
                | Expression::ArrowFunctionExpression(_)
                | Expression::ClassExpression(_)
                | Expression::NewExpression(_)
                | Expression::RegExpLiteral(_)
                | Expression::BinaryExpression(_)
                | Expression::TSAsExpression(_)
                | Expression::TSTypeAssertion(_)
        ),
    }
}

/// A use where a literal type behaves like its primitive (`'open'` like `string`, `5` like
/// `number`): `${status}`, `count + 1`, `count < limit`, `status.length`, `!flag`, the test of
/// `if`, a loop or `?:`, a statement of its own, the value of a `let`, `var` or annotated
/// variable, which widens it again. A comparison, a call argument or a `const` alias sees the
/// literal type.
pub fn keeps_primitive(file: &File<'_>, node: &AstNode<'_>) -> bool {
    let nodes = file.semantic.nodes();
    let span = node.span();
    let parent = nodes.parent_node(node.id());
    match parent.kind() {
        // A tag function may be generic over its values.
        AstKind::TemplateLiteral(_) => !matches!(
            nodes.parent_kind(parent.id()),
            AstKind::TaggedTemplateExpression(_)
        ),
        AstKind::BinaryExpression(binary) => {
            binary.operator.is_numeric_or_string_binary_operator() || binary.operator.is_compare()
        }
        AstKind::UnaryExpression(unary) => unary.operator != UnaryOperator::Delete,
        AstKind::IfStatement(statement) => statement.test.span() == span,
        AstKind::WhileStatement(statement) => statement.test.span() == span,
        AstKind::DoWhileStatement(statement) => statement.test.span() == span,
        AstKind::ForStatement(statement) => statement
            .test
            .as_ref()
            .is_some_and(|test| test.span() == span),
        AstKind::ConditionalExpression(conditional) => conditional.test.span() == span,
        AstKind::ExpressionStatement(_) => true,
        AstKind::VariableDeclarator(declarator) => {
            declarator.type_annotation.is_some()
                || matches!(
                    nodes.parent_kind(parent.id()),
                    AstKind::VariableDeclaration(declaration)
                        if matches!(
                            declaration.kind,
                            VariableDeclarationKind::Let | VariableDeclarationKind::Var
                        )
                )
        }
        kind => kind
            .as_member_expression_kind()
            .is_some_and(|member| member.object().span() == span),
    }
}

/// Another file, or a use of the symbol that is not `keeps_primitive`, may rely on its wide type
/// and stop compiling when a `const` gives it the literal one: `status === 'closed'`, a generic
/// call `pick(status)`, an alias `const current = status`.
pub fn relies_on_wide_type(file: &File<'_>, symbol: SymbolId) -> bool {
    let nodes = file.semantic.nodes();
    is_exported(file, symbol)
        || file
            .semantic
            .scoping()
            .get_resolved_references(symbol)
            .any(|reference| !keeps_primitive(file, nodes.get_node(reference.node_id())))
}

/// Methods that give an array when called on one.
const ARRAY_METHODS: &[&str] = &[
    "map",
    "filter",
    "flatMap",
    "flat",
    "slice",
    "concat",
    "sort",
    "reverse",
    "toSorted",
    "toReversed",
    "toSpliced",
];

/// The value is an array, as its syntax shows: an array literal; one of `ARRAY_METHODS` called on
/// such a value; the global `Array.from(..)`, `Array.of(..)`, `Object.keys(..)`, `.values(..)`,
/// `.entries(..)`; a name annotated as an array or a tuple (`T[]`, `readonly T[]`, lib
/// `Array<T>` / `ReadonlyArray<T>`, also with `null` / `undefined`), or a `const` without an
/// annotation that holds such a value. Also through `?.`, `!`, `as` and `satisfies`. A parameter
/// without a type, a destructured name, an import and another call's result are not seen:
/// `rows.filter(..)` may be a query builder's.
pub fn is_known_array(file: &File<'_>, expression: &Expression<'_>) -> bool {
    is_array(file, expression, &mut HashSet::new())
}

/// `seen` holds the names already followed, so `const a = b; const b = a;` ends.
fn is_array(file: &File<'_>, expression: &Expression<'_>, seen: &mut HashSet<SymbolId>) -> bool {
    if let Some(call) = call_of(expression) {
        let Some(member) = call.callee.get_inner_expression().get_member_expr() else {
            return false;
        };
        let Some(method) = member.static_property_name() else {
            return false;
        };
        let object = member.object().get_inner_expression();
        return ARRAY_METHODS.contains(&method) && is_array(file, object, seen)
            || matches!(method, "from" | "of") && is_global(file, object, "Array")
            || matches!(method, "keys" | "values" | "entries")
                && is_global(file, object, "Object");
    }
    let id = match expression.get_inner_expression() {
        Expression::ArrayExpression(_) => return true,
        Expression::Identifier(id) => id,
        _ => return false,
    };
    let scoping = file.semantic.scoping();
    let Some(symbol) = scoping.get_reference(id.reference_id()).symbol_id() else {
        return false;
    };
    if !seen.insert(symbol) {
        return false;
    }
    let nodes = file.semantic.nodes();
    let declaration = scoping.symbol_declaration(symbol);
    // The annotation of a destructuring pattern types the whole value, not this name.
    let (pattern, annotation) = match nodes.kind(declaration) {
        AstKind::VariableDeclarator(declarator) => (&declarator.id, &declarator.type_annotation),
        AstKind::FormalParameter(parameter) => (&parameter.pattern, &parameter.type_annotation),
        _ => return false,
    };
    if !matches!(pattern, BindingPattern::BindingIdentifier(_)) {
        return false;
    }
    if let Some(annotation) = annotation {
        // Whatever is written to the name later is of this type too.
        return is_array_type(file, &annotation.type_annotation);
    }
    match (nodes.kind(declaration), nodes.parent_kind(declaration)) {
        (AstKind::VariableDeclarator(declarator), AstKind::VariableDeclaration(statement)) => {
            statement.kind.is_const()
                && declarator
                    .init
                    .as_ref()
                    .is_some_and(|init| is_array(file, init, seen))
        }
        _ => false,
    }
}

fn is_array_type(file: &File<'_>, ty: &TSType<'_>) -> bool {
    is_nullable(ty, |ty| {
        let ty = unwrapped_type(file, ty);
        matches!(ty, TSType::TSArrayType(_) | TSType::TSTupleType(_))
            || ["Array", "ReadonlyArray"]
                .into_iter()
                .any(|name| lib_type(file, ty, name).is_some())
    })
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
