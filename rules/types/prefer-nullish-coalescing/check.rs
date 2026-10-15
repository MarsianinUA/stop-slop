use kit::oxc::ast::ast::{
    AssignmentOperator, AssignmentTarget, BinaryOperator, BindingPattern, CallExpression,
    ChainElement, Expression, IdentifierReference, LogicalOperator, TSType, TSTypeAnnotation,
    UnaryOperator, VariableDeclarationKind,
};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

/// How many `const` names deep a boolean value is followed.
const MAX_DEPTH: u32 = 4;

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::LogicalExpression, AstType::AssignmentExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let span = match node.kind() {
            AstKind::LogicalExpression(logical) if logical.operator == LogicalOperator::Or => {
                // `a || b || c` reports once, at the outer link.
                let inner_link = matches!(
                    file.semantic.nodes().parent_kind(node.id()),
                    AstKind::LogicalExpression(parent)
                        if parent.operator == LogicalOperator::Or
                            && parent.left.span() == logical.span
                );
                // `x || undefined` turns `''` and `0` into `undefined`; `??` would do nothing.
                if inner_link
                    || logical.right.is_specific_id("undefined")
                    || is_comparator(&logical.left)
                    || is_boolean(file, &logical.left, 0)
                    || is_boolean(file, &logical.right, 0)
                    || in_boolean_context(file, node)
                    || has_boolean_target(file, node)
                {
                    return;
                }
                logical.span
            }
            AstKind::AssignmentExpression(assignment)
                if assignment.operator == AssignmentOperator::LogicalOr
                    && !is_boolean_target(file, &assignment.left)
                    && !is_boolean(file, &assignment.right, 0) =>
            {
                assignment.span
            }
            _ => return,
        };
        cx.report(span);
    }
}

/// Only the truthiness of the value counts: the test of `if`, a loop or `?:`, the operand of
/// `!`, the left of `&&`, the argument of `Boolean(..)`. The value passes up through a branch of
/// `?:` and through `&&` (as its right), `||` and `??`.
fn in_boolean_context(file: &File<'_>, node: &AstNode<'_>) -> bool {
    let nodes = file.semantic.nodes();
    let mut current = node;
    loop {
        let span = current.span();
        let parent = nodes.parent_node(current.id());
        match parent.kind() {
            AstKind::IfStatement(statement) => return statement.test.span() == span,
            AstKind::WhileStatement(statement) => return statement.test.span() == span,
            AstKind::DoWhileStatement(statement) => return statement.test.span() == span,
            AstKind::ForStatement(statement) => {
                return statement
                    .test
                    .as_ref()
                    .is_some_and(|test| test.span() == span);
            }
            AstKind::ConditionalExpression(conditional) if conditional.test.span() == span => {
                return true;
            }
            AstKind::LogicalExpression(logical)
                if logical.operator == LogicalOperator::And && logical.left.span() == span =>
            {
                return true;
            }
            AstKind::ConditionalExpression(_) | AstKind::LogicalExpression(_) => current = parent,
            AstKind::UnaryExpression(unary) => return unary.operator == UnaryOperator::LogicalNot,
            AstKind::CallExpression(call) => return call.callee.is_specific_id("Boolean"),
            _ => return false,
        }
    }
}

/// A boolean by its syntax or by its name: `true`, a comparison, `!x`, `isReady()`,
/// `items.includes(x)`, `x ?? false`, `&&` or `||` of booleans, a `const` set to a boolean, a
/// variable or parameter typed `boolean`.
fn is_boolean(file: &File<'_>, expression: &Expression<'_>, depth: u32) -> bool {
    match expression.get_inner_expression() {
        Expression::BooleanLiteral(_) => true,
        Expression::BinaryExpression(binary) => {
            binary.operator.is_equality()
                || binary.operator.is_compare()
                || binary.operator.is_relational()
        }
        Expression::UnaryExpression(unary) => unary.operator == UnaryOperator::LogicalNot,
        Expression::LogicalExpression(logical) if logical.operator == LogicalOperator::Coalesce => {
            matches!(
                logical.right.get_inner_expression(),
                Expression::BooleanLiteral(_)
            )
        }
        Expression::LogicalExpression(logical) => {
            is_boolean(file, &logical.left, depth) && is_boolean(file, &logical.right, depth)
        }
        Expression::Identifier(id) => {
            has_boolean_name(&id.name) || is_boolean_binding(file, id, depth)
        }
        Expression::StaticMemberExpression(member) => has_boolean_name(&member.property.name),
        Expression::CallExpression(call) => is_boolean_call(call),
        Expression::ChainExpression(chain) => match &chain.expression {
            ChainElement::StaticMemberExpression(member) => has_boolean_name(&member.property.name),
            ChainElement::CallExpression(call) => is_boolean_call(call),
            _ => false,
        },
        _ => false,
    }
}

/// The left of `||=`: `done ||= ..` with `let done: boolean`, `this.isDirty ||= ..`.
fn is_boolean_target(file: &File<'_>, target: &AssignmentTarget<'_>) -> bool {
    match target {
        AssignmentTarget::AssignmentTargetIdentifier(id) => {
            has_boolean_name(&id.name) || is_boolean_binding(file, id, 0)
        }
        AssignmentTarget::StaticMemberExpression(member) => has_boolean_name(&member.property.name),
        _ => false,
    }
}

fn is_boolean_call(call: &CallExpression<'_>) -> bool {
    match call.callee.get_inner_expression() {
        Expression::Identifier(id) => has_boolean_name(&id.name),
        Expression::StaticMemberExpression(member) => {
            let name = member.property.name.as_str();
            has_boolean_name(name)
                || matches!(
                    name,
                    "includes" | "some" | "every" | "startsWith" | "endsWith" | "test"
                )
        }
        _ => false,
    }
}

/// `isReady`, `hasItems`, `canRetry`, `shouldSkip`; not `island` or `hash`.
fn has_boolean_name(name: &str) -> bool {
    ["is", "has", "can", "should"].into_iter().any(|prefix| {
        name.strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_uppercase()))
    })
}

/// `const changed = a !== b`, `let done: boolean`, `(flag: boolean)`.
fn is_boolean_binding(file: &File<'_>, id: &IdentifierReference<'_>, depth: u32) -> bool {
    if depth >= MAX_DEPTH {
        return false;
    }
    let scoping = file.semantic.scoping();
    let Some(symbol) = scoping.get_reference(id.reference_id()).symbol_id() else {
        return false;
    };
    let nodes = file.semantic.nodes();
    let declaration = scoping.symbol_declaration(symbol);
    match nodes.kind(declaration) {
        AstKind::VariableDeclarator(declarator) if declarator.type_annotation.is_some() => {
            is_boolean_type(declarator.type_annotation.as_deref())
        }
        AstKind::VariableDeclarator(declarator) => {
            matches!(
                nodes.parent_kind(declaration),
                AstKind::VariableDeclaration(statement)
                    if statement.kind == VariableDeclarationKind::Const
            ) && matches!(declarator.id, BindingPattern::BindingIdentifier(_))
                && declarator
                    .init
                    .as_ref()
                    .is_some_and(|init| is_boolean(file, init, depth + 1))
        }
        AstKind::FormalParameter(parameter) => {
            is_boolean_type(parameter.type_annotation.as_deref())
        }
        _ => false,
    }
}

fn is_boolean_type(annotation: Option<&TSTypeAnnotation<'_>>) -> bool {
    annotation
        .is_some_and(|annotation| matches!(annotation.type_annotation, TSType::TSBooleanKeyword(_)))
}

/// `a.rank - b.rank || ..`, `a.localeCompare(b) || ..`: a sort comparator moves to the next key
/// on `0`, which `??` would keep.
fn is_comparator(expression: &Expression<'_>) -> bool {
    match expression.get_inner_expression() {
        Expression::BinaryExpression(binary) => binary.operator == BinaryOperator::Subtraction,
        Expression::CallExpression(call) => {
            let name = match call.callee.get_inner_expression() {
                Expression::Identifier(id) => id.name.as_str(),
                Expression::StaticMemberExpression(member) => member.property.name.as_str(),
                _ => return false,
            };
            name.as_bytes()
                .windows(b"compare".len())
                .any(|window| window.eq_ignore_ascii_case(b"compare"))
        }
        Expression::LogicalExpression(logical) if logical.operator == LogicalOperator::Or => {
            is_comparator(&logical.left)
        }
        _ => false,
    }
}

/// `const ok: boolean = a || b`, `return a || b` in a function typed `: boolean`, the body of an
/// arrow typed `: boolean`.
fn has_boolean_target(file: &File<'_>, node: &AstNode<'_>) -> bool {
    let nodes = file.semantic.nodes();
    match nodes.parent_kind(node.id()) {
        AstKind::VariableDeclarator(declarator) => {
            is_boolean_type(declarator.type_annotation.as_deref())
        }
        AstKind::ArrowFunctionExpression(arrow) => is_boolean_type(arrow.return_type.as_deref()),
        AstKind::ReturnStatement(_) => nodes
            .ancestors(node.id())
            .find_map(|ancestor| match ancestor.kind() {
                AstKind::Function(function) => Some(function.return_type.as_deref()),
                AstKind::ArrowFunctionExpression(arrow) => Some(arrow.return_type.as_deref()),
                _ => None,
            })
            .is_some_and(is_boolean_type),
        _ => false,
    }
}
