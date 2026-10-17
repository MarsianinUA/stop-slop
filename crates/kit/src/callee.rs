use oxc_ast::AstKind;
use oxc_ast::ast::{CallExpression, Class, ClassElement, Expression, MethodDefinitionKind};
use oxc_semantic::{AstNode, NodeId};

use crate::{File, lib_type};

/// The function of this file that the call runs: a function declared with a body, a `const`
/// holding a function or an arrow, or `this.m(..)` - a method with a body or a function field of
/// the class whose `this` the call sees. A binding that is written to may hold another function.
pub fn local_callee<'a>(file: &File<'a>, call: &CallExpression<'a>) -> Option<&'a AstNode<'a>> {
    let nodes = file.semantic.nodes();
    let scoping = file.semantic.scoping();
    match call.callee.get_inner_expression() {
        Expression::Identifier(id) => {
            let symbol = scoping.get_reference(id.reference_id()).symbol_id()?;
            let declaration = scoping.symbol_declaration(symbol);
            let callee = match nodes.kind(declaration) {
                AstKind::Function(function) if function.body.is_some() => {
                    nodes.get_node(declaration)
                }
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
            let name = member.property.name.as_str();
            this_class(file, call.node_id.get())?
                .body
                .body
                .iter()
                .find_map(|element| match element {
                    ClassElement::MethodDefinition(method)
                        if !method.r#static
                            && method.kind == MethodDefinitionKind::Method
                            && method.value.body.is_some()
                            && method.key.static_name().is_some_and(|key| key == name) =>
                    {
                        Some(nodes.get_node(method.value.node_id.get()))
                    }
                    ClassElement::PropertyDefinition(field)
                        if !field.r#static
                            && field.key.static_name().is_some_and(|key| key == name) =>
                    {
                        function_value(file, field.value.as_ref()?)
                    }
                    _ => None,
                })
        }
        _ => None,
    }
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
