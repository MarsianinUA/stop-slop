use oxc_ast::AstKind;
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
