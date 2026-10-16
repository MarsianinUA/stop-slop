use kit::oxc::ast::ast::{
    BindingPattern, Expression, FormalParameter, LogicalOperator, MethodDefinitionKind,
    PropertyKey, TSAccessibility, TSLiteral, TSType, UnaryOperator,
};
use kit::oxc::semantic::NodeId;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::Function, AstType::ArrowFunctionExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let params = match node.kind() {
            // Without a body it is a signature: an overload, an `abstract` method, a `declare`.
            AstKind::Function(function) if function.body.is_some() => &function.params,
            AstKind::ArrowFunctionExpression(arrow) => &arrow.params,
            _ => return,
        };
        if !params.items.iter().any(is_boolean_parameter)
            || is_ambient(file, node.id())
            || is_signature_given(file, node)
        {
            return;
        }
        let scoping = file.semantic.scoping();
        for parameter in &params.items {
            // A parameter property is a field; a decorated parameter is filled by the framework.
            if !parameter.has_modifier()
                && parameter.decorators.is_empty()
                && is_boolean_parameter(parameter)
                && let BindingPattern::BindingIdentifier(id) = &parameter.pattern
                && let Some(symbol) = id.symbol_id.get()
                && scoping
                    .get_resolved_references(symbol)
                    .any(|reference| picks_path(file, reference.node_id()))
            {
                cx.report(parameter.span).arg("name", id.name);
            }
        }
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

/// Through `!`, `&&`, `||` and type assertions the reference is the test of `if`, `?:` or a loop,
/// or the left side of a `&&` / `||` that is a whole statement (`debug && send(event);`). A value
/// passed on, stored or returned picks no path.
fn picks_path(file: &File<'_>, reference: NodeId) -> bool {
    let nodes = file.semantic.nodes();
    let mut current = outer_assertion(file, nodes.get_node(reference));
    loop {
        let parent = nodes.parent_node(current.id());
        let span = current.span();
        match parent.kind() {
            AstKind::UnaryExpression(unary) if unary.operator == UnaryOperator::LogicalNot => {}
            AstKind::LogicalExpression(logical)
                if logical.operator != LogicalOperator::Coalesce =>
            {
                if logical.left.span() == span
                    && matches!(
                        nodes.parent_kind(parent.id()),
                        AstKind::ExpressionStatement(_)
                    )
                {
                    return true;
                }
            }
            AstKind::IfStatement(statement) => return statement.test.span() == span,
            AstKind::ConditionalExpression(conditional) => return conditional.test.span() == span,
            AstKind::WhileStatement(statement) => return statement.test.span() == span,
            AstKind::DoWhileStatement(statement) => return statement.test.span() == span,
            AstKind::ForStatement(statement) => {
                return statement
                    .test
                    .as_ref()
                    .is_some_and(|test| test.span() == span);
            }
            _ => return false,
        }
        current = outer_assertion(file, parent);
    }
}

/// Someone else sets the signature: the caller of a callback, the property of a setter, the base
/// class of an `override`, the interface of a class that `implements` one (for its public
/// members), the type of the variable or field that holds the function
/// (`const handler: RequestHandler = (req, res, next) => ..`), the type it `satisfies` or is
/// asserted to, the return type of the function that returns it
/// (`function audit(): RequestHandler { return (req, res, next) => .. }`).
fn is_signature_given(file: &File<'_>, function: &AstNode<'_>) -> bool {
    if is_callback(file, function) {
        return true;
    }
    let nodes = file.semantic.nodes();
    let parent = nodes.parent_node(function.id());
    match parent.kind() {
        AstKind::VariableDeclarator(declarator) => declarator.type_annotation.is_some(),
        AstKind::TSSatisfiesExpression(_)
        | AstKind::TSAsExpression(_)
        | AstKind::TSTypeAssertion(_) => true,
        // The body of an arrow: `(prefix): RequestHandler => (req, res, next) => ..`.
        AstKind::ArrowFunctionExpression(arrow) => arrow.return_type.is_some(),
        AstKind::ReturnStatement(_) => nodes
            .ancestor_kinds(parent.id())
            .find_map(|kind| match kind {
                AstKind::Function(outer) => Some(outer.return_type.is_some()),
                AstKind::ArrowFunctionExpression(outer) => Some(outer.return_type.is_some()),
                _ => None,
            })
            .unwrap_or(false),
        AstKind::PropertyDefinition(field) => {
            field.type_annotation.is_some()
                || !field.r#static
                    && is_public(field.accessibility, &field.key)
                    && implements(file, parent)
        }
        AstKind::MethodDefinition(method) => {
            method.kind == MethodDefinitionKind::Set
                || method.r#override
                || method.kind == MethodDefinitionKind::Method
                    && !method.r#static
                    && is_public(method.accessibility, &method.key)
                    && implements(file, parent)
        }
        _ => false,
    }
}

fn is_public(accessibility: Option<TSAccessibility>, key: &PropertyKey<'_>) -> bool {
    !key.is_private_identifier()
        && !matches!(
            accessibility,
            Some(TSAccessibility::Private | TSAccessibility::Protected)
        )
}

/// member -> `ClassBody` -> `Class` with `implements`.
fn implements(file: &File<'_>, member: &AstNode<'_>) -> bool {
    let nodes = file.semantic.nodes();
    matches!(
        nodes.parent_kind(nodes.parent_id(member.id())),
        AstKind::Class(class) if !class.implements.is_empty()
    )
}
