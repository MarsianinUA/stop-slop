use std::collections::HashSet;

use kit::oxc::ast::ast::{
    AccessorProperty, Function, FunctionType, PropertyDefinition, PropertyKind,
};
use kit::oxc::semantic::NodeId;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let mut candidates = Vec::new();
        let mut binds = Vec::new();
        for node in file.semantic.nodes().iter() {
            match node.kind() {
                // A generator has no arrow form; `this: T` declares the `this` the caller binds;
                // a name says the function is meant as one: `function Pool() {}` is constructed
                // with `new`, `function retry() {}` calls itself, and a stack trace or `.name`
                // shows it.
                AstKind::Function(function)
                    if function.r#type == FunctionType::FunctionExpression
                        && !function.generator
                        && function.this_param.is_none()
                        && function.id.is_none() =>
                {
                    candidates.push((node, function));
                }
                AstKind::ThisExpression(_) | AstKind::Super(_) | AstKind::NewTarget(_) => {
                    binds.push(node);
                }
                _ => {}
            }
        }
        if candidates.is_empty() {
            return;
        }
        // Functions whose own `this`, `arguments`, `super` or `new.target` the code uses: an
        // arrow has none of them.
        let bound: HashSet<NodeId> = binds
            .into_iter()
            .chain(global_references(file, "arguments"))
            .filter_map(|node| binder(file, node))
            .collect();
        for (node, function) in candidates {
            if bound.contains(&node.id()) || !is_callback(file, node) || is_class_slot(file, node) {
                continue;
            }
            let reported = cx.report(function.span);
            if let Some(edits) = arrow(file, function) {
                reported.fix(edits);
            }
        }
    }
}

/// The function whose `this` the node reads: the nearest one up that is not an arrow. A static
/// block or a field initializer has a `this` of its own.
fn binder(file: &File<'_>, node: &AstNode<'_>) -> Option<NodeId> {
    let span = node.span();
    for ancestor in file.semantic.nodes().ancestors(node.id()) {
        match ancestor.kind() {
            AstKind::Function(_) => return Some(ancestor.id()),
            AstKind::StaticBlock(_) => return None,
            AstKind::PropertyDefinition(PropertyDefinition {
                value: Some(value), ..
            })
            | AstKind::AccessorProperty(AccessorProperty {
                value: Some(value), ..
            }) if value.span().contains_inclusive(span) => {
                return None;
            }
            _ => {}
        }
    }
    None
}

/// A method of an object (`{ save() {} }`, `{ get total() {} }`), or the value of a key named like
/// a class, which its user may construct with `new`:
/// `jest.mock('pg', () => ({ Pool: function () {..} }))`.
fn is_class_slot(file: &File<'_>, node: &AstNode<'_>) -> bool {
    matches!(file.semantic.nodes().parent_kind(node.id()), AstKind::ObjectProperty(property)
        if property.method
            || property.kind != PropertyKind::Init
            || property
                .key
                .static_name()
                .is_some_and(|name| name.starts_with(char::is_uppercase)))
}

/// `function (a): T {` -> `(a): T => {`: `=>` goes right after the head, on its last line. `None`
/// when more than the keywords stands before the parameters: type parameters (`<T>(` reads as JSX
/// in a `.tsx` file) or a comment.
fn arrow(file: &File<'_>, function: &Function<'_>) -> Option<[Edit; 2]> {
    let head = Span::new(function.span.start, function.params.span.start);
    let keywords: Vec<&str> = head.source_text(file.text).split_whitespace().collect();
    let prefix = match keywords.as_slice() {
        ["function"] => "",
        ["async", "function"] => "async ",
        _ => return None,
    };
    let end = function
        .return_type
        .as_ref()
        .map_or(function.params.span.end, |annotation| annotation.span.end);
    Some([
        Edit {
            span: head,
            text: prefix.into(),
        },
        Edit {
            span: Span::empty(end),
            text: " =>".into(),
        },
    ])
}
