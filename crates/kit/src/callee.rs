use oxc_ast::AstKind;
use oxc_ast::ast::{
    CallExpression, ChainElement, Class, ClassElement, Expression, MethodDefinitionKind,
    PropertyKey,
};
use oxc_semantic::{AstNode, NodeId};

use crate::{File, lib_type};

/// The call the expression is, also through `?.` and the wrappers `as`, `!`, `satisfies` and
/// `<T>`: `save(id)`, `client?.send(..)`, `load(id) as Promise<Order>`.
pub fn call_of<'b, 'a>(expression: &'b Expression<'a>) -> Option<&'b CallExpression<'a>> {
    match expression.get_inner_expression() {
        Expression::CallExpression(call) => Some(call),
        Expression::ChainExpression(chain) => match &chain.expression {
            ChainElement::CallExpression(call) => Some(call),
            _ => None,
        },
        _ => None,
    }
}

/// The function of this file that the call runs: a function declared with a body, a `const`
/// holding a function or an arrow, or `this.m(..)` / `this.#m(..)` - a method with a body or a
/// function field of the class whose `this` the call sees. A binding that is written to may hold
/// another function.
pub fn local_callee<'a>(file: &File<'a>, call: &CallExpression<'a>) -> Option<&'a AstNode<'a>> {
    let nodes = file.semantic.nodes();
    let scoping = file.semantic.scoping();
    match call.callee.get_inner_expression() {
        Expression::Identifier(id) => {
            let symbol = scoping.get_reference(id.reference_id()).symbol_id()?;
            let declaration = scoping.symbol_declaration(symbol);
            let callee = match nodes.kind(declaration) {
                // An overloaded function: the symbol is declared by its first signature, the
                // call runs the declaration with a body.
                AstKind::Function(function) if function.body.is_none() => scoping
                    .symbol_redeclarations(symbol)
                    .iter()
                    .map(|redeclaration| nodes.get_node(redeclaration.declaration))
                    .find(|node| {
                        matches!(node.kind(), AstKind::Function(function) if function.body.is_some())
                    })?,
                AstKind::Function(_) => nodes.get_node(declaration),
                AstKind::VariableDeclarator(declarator) => {
                    function_value(file, declarator.init.as_ref()?)?
                }
                _ => return None,
            };
            let is_written = scoping
                .get_resolved_references(symbol)
                .any(|reference| reference.is_write());
            (!is_written).then_some(callee)
        }
        Expression::StaticMemberExpression(member)
            if matches!(member.object, Expression::ThisExpression(_)) =>
        {
            this_member(file, call, |key| {
                key.static_name()
                    .is_some_and(|key| key == member.property.name.as_str())
            })
        }
        Expression::PrivateFieldExpression(member)
            if matches!(member.object, Expression::ThisExpression(_)) =>
        {
            let name = member.field.name;
            this_member(
                file,
                call,
                |key| matches!(key, PropertyKey::PrivateIdentifier(id) if id.name == name),
            )
        }
        _ => None,
    }
}

fn this_member<'a>(
    file: &File<'a>,
    call: &CallExpression<'a>,
    is_key: impl Fn(&PropertyKey<'a>) -> bool,
) -> Option<&'a AstNode<'a>> {
    let nodes = file.semantic.nodes();
    this_class(file, call.node_id.get())?
        .body
        .body
        .iter()
        .find_map(|element| match element {
            ClassElement::MethodDefinition(method)
                if !method.r#static
                    && method.kind == MethodDefinitionKind::Method
                    && method.value.body.is_some()
                    && is_key(&method.key) =>
            {
                Some(nodes.get_node(method.value.node_id.get()))
            }
            ClassElement::PropertyDefinition(field) if !field.r#static && is_key(&field.key) => {
                function_value(file, field.value.as_ref()?)
            }
            _ => None,
        })
}

/// A function or arrow whose call gives a promise: `async`, or the lib `Promise<..>` as its return
/// type. An async generator gives an iterator: its call starts no work.
pub fn returns_promise(file: &File<'_>, function: &AstNode<'_>) -> bool {
    let (is_async, return_type) = match function.kind() {
        AstKind::Function(function) => (
            function.r#async && !function.generator,
            &function.return_type,
        ),
        AstKind::ArrowFunctionExpression(arrow) => (arrow.r#async, &arrow.return_type),
        _ => return false,
    };
    is_async
        || return_type
            .as_ref()
            .is_some_and(|ty| lib_type(file, &ty.type_annotation, "Promise").is_some())
}

fn function_value<'a>(file: &File<'a>, value: &Expression<'a>) -> Option<&'a AstNode<'a>> {
    let id = match value.get_inner_expression() {
        Expression::ArrowFunctionExpression(arrow) => arrow.node_id.get(),
        Expression::FunctionExpression(function) => function.node_id.get(),
        _ => return None,
    };
    Some(file.semantic.nodes().get_node(id))
}

/// The class whose instance is `this` at the node: up through arrows to a non-static method or
/// field. Any other function in between has its own `this`, also one inside a field's arrow.
pub fn this_class<'a>(file: &File<'a>, node: NodeId) -> Option<&'a Class<'a>> {
    let nodes = file.semantic.nodes();
    for ancestor in nodes.ancestors(node) {
        match ancestor.kind() {
            AstKind::Function(_) => {
                let is_member = match nodes.parent_kind(ancestor.id()) {
                    AstKind::MethodDefinition(method) => !method.r#static,
                    AstKind::PropertyDefinition(field) => !field.r#static,
                    _ => false,
                };
                if !is_member {
                    return None;
                }
            }
            AstKind::MethodDefinition(method) if method.r#static => return None,
            AstKind::PropertyDefinition(field) if field.r#static => return None,
            AstKind::StaticBlock(_) => return None,
            AstKind::Class(class) => return Some(class),
            _ => {}
        }
    }
    None
}
