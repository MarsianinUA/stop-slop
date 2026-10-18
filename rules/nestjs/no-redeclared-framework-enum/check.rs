use std::collections::BTreeMap;

use kit::oxc::ast::ast::{BindingPattern, Expression};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// An enum of the framework and the module that exports it.
    enums: BTreeMap<String, String>,
}

impl Rule for Check {
    const NODES: &'static [AstType] = &[
        AstType::TSEnumDeclaration,
        AstType::TSTypeAliasDeclaration,
        AstType::Class,
        AstType::VariableDeclarator,
    ];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let nodes = file.semantic.nodes();
        // From the keyword to the name: `enum HttpStatus`, `const HttpStatus`.
        let (start, name) = match node.kind() {
            AstKind::TSEnumDeclaration(declaration) => {
                (declaration.span.start, declaration.id.span)
            }
            AstKind::TSTypeAliasDeclaration(alias) => (alias.span.start, alias.id.span),
            AstKind::Class(class) => {
                let Some(id) = &class.id else {
                    return;
                };
                (
                    start_after_decorators(file, class.span, &class.decorators),
                    id.span,
                )
            }
            AstKind::VariableDeclarator(declarator) => {
                let BindingPattern::BindingIdentifier(id) = &declarator.id else {
                    return;
                };
                if !declarator
                    .init
                    .as_ref()
                    .is_some_and(|init| is_object(file, init))
                {
                    return;
                }
                (nodes.parent_node(node.id()).span().start, id.span)
            }
            _ => return,
        };
        let text = name.source_text(file.text);
        let Some(module) = self.enums.get(text) else {
            return;
        };
        // `declare module '@nestjs/common' { .. }` describes the framework's own enum.
        if is_ambient(file, node.id()) {
            return;
        }
        cx.report(Span::new(start, name.end))
            .arg("name", text)
            .arg("module", module);
    }
}

/// An object literal, also under `as const`, `satisfies` and the global `Object.freeze(..)`.
fn is_object(file: &File<'_>, value: &Expression<'_>) -> bool {
    match value.get_inner_expression() {
        Expression::ObjectExpression(_) => true,
        Expression::CallExpression(call) => {
            call.callee.get_member_expr().is_some_and(|member| {
                member.static_property_name() == Some("freeze")
                    && is_global(file, member.object().get_inner_expression(), "Object")
            }) && call
                .arguments
                .first()
                .and_then(|argument| argument.as_expression())
                .is_some_and(|argument| is_object(file, argument))
        }
        _ => false,
    }
}
