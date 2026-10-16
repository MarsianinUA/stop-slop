use std::borrow::Cow;

use oxc_ast::ast::Expression;
use oxc_ast::{AstKind, MemberExpressionKind};
use oxc_semantic::AstNode;

use crate::File;

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
