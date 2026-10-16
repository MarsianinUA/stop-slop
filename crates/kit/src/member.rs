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
/// deleted: `x.a = ..`, `x[k] += ..`, `x.a.b++`, `delete x?.a`, also the target of
/// `for (x.a of ..)` / `for (x.a in ..)` or of a destructuring assignment (`[x.a] = pair`): the
/// span of the assignment, update or `delete`, in a loop the span of its target.
pub fn member_write(file: &File<'_>, reference: NodeId) -> Option<Span> {
    let nodes = file.semantic.nodes();
    let mut current = outer_assertion(file, nodes.get_node(reference));
    let mut parent = nodes.parent_node(current.id());
    let mut is_member = false;
    loop {
        match parent.kind().as_member_expression_kind() {
            // The key of `o[x]` is not the object that changes.
            Some(member) if member.object().span() != current.span() => return None,
            Some(_) => is_member = true,
            None if matches!(parent.kind(), AstKind::ChainExpression(_)) => {}
            None => break,
        }
        current = outer_assertion(file, parent);
        parent = nodes.parent_node(current.id());
    }
    if !is_member {
        return None;
    }
    let target = current.span();
    match parent.kind() {
        AstKind::AssignmentExpression(assignment) if assignment.left.span() == target => {
            Some(parent.span())
        }
        AstKind::UpdateExpression(_) => Some(parent.span()),
        AstKind::UnaryExpression(unary) if unary.operator == UnaryOperator::Delete => {
            Some(parent.span())
        }
        AstKind::ForOfStatement(statement) if statement.left.span() == target => Some(target),
        AstKind::ForInStatement(statement) if statement.left.span() == target => Some(target),
        AstKind::AssignmentTargetWithDefault(element) if element.binding.span() != target => None,
        // A computed key reads: `({ [x.a]: y } = o)`.
        AstKind::AssignmentTargetPropertyProperty(property)
            if property.binding.span() != target =>
        {
            None
        }
        kind if is_pattern(kind) => {
            let mut pattern = parent;
            loop {
                let above = nodes.parent_node(pattern.id());
                if !is_pattern(above.kind()) {
                    // The assignment, or the pattern of a `for..of` / `for..in`.
                    let is_assignment = matches!(above.kind(), AstKind::AssignmentExpression(_));
                    return Some(if is_assignment { above } else { pattern }.span());
                }
                pattern = above;
            }
        }
        _ => None,
    }
}

fn is_pattern(kind: AstKind<'_>) -> bool {
    matches!(
        kind,
        AstKind::ArrayAssignmentTarget(_)
            | AstKind::ObjectAssignmentTarget(_)
            | AstKind::AssignmentTargetRest(_)
            | AstKind::AssignmentTargetWithDefault(_)
            | AstKind::AssignmentTargetPropertyProperty(_)
    )
}

/// The classes whose instances `MUTATORS` change in place.
pub const COLLECTIONS: [&str; 5] = ["Array", "Map", "Set", "WeakMap", "WeakSet"];

/// Methods that change a `Map`, `Set`, `WeakMap`, `WeakSet` or array in place: the language's
/// own, not a taste to configure.
const MUTATORS: &[&str] = &[
    "set",
    "delete",
    "clear",
    "add",
    "push",
    "pop",
    "shift",
    "unshift",
    "splice",
    "sort",
    "reverse",
    "fill",
    "copyWithin",
];

/// The call when the reference (through `!`, `as`, `satisfies`, `<T>`) is the receiver of one of
/// those methods: `items.push(..)`, `seen.add(..)`. The receiver's type is not seen.
pub fn mutator_call(file: &File<'_>, reference: NodeId) -> Option<Span> {
    let nodes = file.semantic.nodes();
    let receiver = outer_assertion(file, nodes.get_node(reference));
    let AstKind::StaticMemberExpression(member) = nodes.parent_kind(receiver.id()) else {
        return None;
    };
    match nodes.parent_kind(nodes.parent_id(receiver.id())) {
        AstKind::CallExpression(call)
            if call.callee.span() == member.span
                && MUTATORS.contains(&member.property.name.as_str()) =>
        {
            Some(call.span)
        }
        _ => None,
    }
}
