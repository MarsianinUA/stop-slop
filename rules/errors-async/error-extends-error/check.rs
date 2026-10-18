use std::collections::HashSet;

use kit::oxc::ast::ast::{Class, Expression};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::Class];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let AstKind::Class(class) = node.kind() else {
            return;
        };
        let Some((name, span)) = error_name(file, node, class) else {
            return;
        };
        // `declare class` describes a class someone else wrote.
        if !is_ambient(file, node.id()) && !may_extend_error(file, class) {
            cx.report(span).arg("name", name);
        }
    }
}

/// The class's own name if it names an error, else the variable it is the whole value of:
/// `const NotFoundError = class {}`, `const NotFoundError = class NotFound {}`.
fn error_name<'a>(
    file: &File<'a>,
    node: &AstNode<'a>,
    class: &'a Class<'a>,
) -> Option<(&'a str, Span)> {
    if let Some(id) = class.id.as_ref().filter(|id| is_error_name(&id.name)) {
        return Some((id.name.as_str(), id.span));
    }
    let value = outer_assertion(file, node);
    match file.semantic.nodes().parent_kind(value.id()) {
        AstKind::VariableDeclarator(declarator) => {
            let id = declarator.id.get_binding_identifier()?;
            is_error_name(&id.name).then_some((id.name.as_str(), id.span))
        }
        _ => None,
    }
}

/// `NotFoundError`, `HTTPError`: `Error` is the last of two or more words.
fn is_error_name(name: &str) -> bool {
    matches!(words(name).as_slice(), [_, .., last] if *last == "Error")
}

/// The class extends something, and it is not a class of this file that extends nothing: an
/// imported or global base may be an `Error`, the syntax does not show it.
fn may_extend_error<'a>(file: &File<'a>, mut class: &'a Class<'a>) -> bool {
    let scoping = file.semantic.scoping();
    // `seen` ends a cycle of `extends`, which TypeScript rejects anyway.
    let mut seen = HashSet::new();
    loop {
        let Some(heritage) = &class.heritage else {
            return false;
        };
        let Expression::Identifier(id) = heritage.expression.get_inner_expression() else {
            return true;
        };
        let Some(symbol) = scoping.get_reference(id.reference_id()).symbol_id() else {
            return true;
        };
        if !seen.insert(symbol) {
            return true;
        }
        let declaration = scoping.symbol_declaration(symbol);
        class = match file.semantic.nodes().kind(declaration) {
            AstKind::Class(base) => base,
            AstKind::VariableDeclarator(declarator) => match declarator
                .init
                .as_ref()
                .map(Expression::get_inner_expression)
            {
                Some(Expression::ClassExpression(base)) => base,
                _ => return true,
            },
            _ => return true,
        };
    }
}
