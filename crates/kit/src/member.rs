use std::borrow::Cow;

use oxc_ast::ast::{Expression, UnaryOperator};
use oxc_ast::{AstKind, MemberExpressionKind};
use oxc_semantic::{AstNode, NodeId};
use oxc_span::{GetSpan, Span};

use crate::{File, outer_assertion};

/// What a node reaches of an object by name. The object's type is not seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberUse<'a> {
    /// `x` for `o.x`, `o['x']` and a key of a destructuring pattern or an assignment target
    /// (`const { x } = o`, `({ x } = o)`); `#x` for `o.#x`.
    Name(Cow<'a, str>),
    /// `this[key]`, also through `as` and `!`: any member of the class.
    Any,
}

/// `None` for other nodes and for `o[key]` on an object other than `this`.
pub fn member_use<'a>(file: &File<'a>, node: &AstNode<'a>) -> Option<MemberUse<'a>> {
    let name = match node.kind() {
        AstKind::BindingProperty(property) => property.key.static_name(),
        AstKind::AssignmentTargetPropertyProperty(property) => property.name.static_name(),
        AstKind::AssignmentTargetPropertyIdentifier(property) => {
            Some(Cow::Borrowed(property.binding.name.as_str()))
        }
        kind => {
            let member = kind.as_member_expression_kind()?;
            let name = match &member {
                MemberExpressionKind::PrivateField(member) => {
                    Some(Cow::Borrowed(member.field.span.source_text(file.text)))
                }
                _ => member
                    .static_property_name()
                    .map(|name| Cow::Borrowed(name.as_str())),
            };
            if name.is_none()
                && matches!(
                    member.object().get_inner_expression(),
                    Expression::ThisExpression(_)
                )
            {
                return Some(MemberUse::Any);
            }
            name
        }
    };
    name.map(MemberUse::Name)
}

/// The write when the reference is the root of a member chain that is assigned, updated or
/// deleted: `x.a = ..`, `x[k] += ..`, `x.a.b++`, `delete x.a`.
pub fn member_write(file: &File<'_>, reference: NodeId) -> Option<Span> {
    let nodes = file.semantic.nodes();
    let mut current = outer_assertion(file, nodes.get_node(reference));
    let mut parent = nodes.parent_node(current.id());
    let mut is_member = false;
    while let Some(member) = parent.kind().as_member_expression_kind() {
        // The key of `o[x]` is not the object that changes.
        if member.object().span() != current.span() {
            return None;
        }
        is_member = true;
        current = outer_assertion(file, parent);
        parent = nodes.parent_node(current.id());
    }
    let is_write = is_member
        && match parent.kind() {
            AstKind::AssignmentExpression(assignment) => assignment.left.span() == current.span(),
            AstKind::UpdateExpression(_) => true,
            AstKind::UnaryExpression(unary) => unary.operator == UnaryOperator::Delete,
            _ => false,
        };
    is_write.then(|| parent.span())
}
