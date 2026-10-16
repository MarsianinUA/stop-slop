use oxc_ast::AstKind;
use oxc_ast::ast::{MethodDefinitionKind, PropertyKey, TSAccessibility};
use oxc_semantic::AstNode;
use oxc_span::GetSpan;

use crate::File;

/// A function or arrow handed to someone else to call: an argument of a call or `new`, the value
/// of a JSX attribute (`onClick={() => save()}`) or of an object property.
pub fn is_callback(file: &File<'_>, function: &AstNode<'_>) -> bool {
    let nodes = file.semantic.nodes();
    let span = function.span();
    match nodes.parent_kind(function.id()) {
        AstKind::CallExpression(call) => call.arguments.iter().any(|arg| arg.span() == span),
        AstKind::NewExpression(new) => new.arguments.iter().any(|arg| arg.span() == span),
        AstKind::JSXExpressionContainer(_) => matches!(
            nodes.parent_kind(nodes.parent_id(function.id())),
            AstKind::JSXAttribute(_)
        ),
        AstKind::ObjectProperty(property) => property.value.span() == span,
        _ => false,
    }
}

/// Someone else sets the signature: the caller of a callback, the property of a setter, the base
/// class of an `override`, the interface of a class that `implements` one (for its public
/// members), the type of the variable or field that holds the function
/// (`const handler: RequestHandler = (req, res, next) => ..`), the type it `satisfies` or is
/// asserted to, the return type of the function that returns it
/// (`function audit(): RequestHandler { return (req, res, next) => .. }`).
pub fn is_signature_given(file: &File<'_>, function: &AstNode<'_>) -> bool {
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
