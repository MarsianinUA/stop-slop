use std::collections::BTreeMap;

use kit::oxc::ast::ast::{BindingPattern, Expression, TSType};
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
        // From the keyword to the name: `enum HttpStatus`, `const HttpStatus`.
        let (start, name) = match node.kind() {
            AstKind::TSEnumDeclaration(declaration) => {
                (declaration.span.start, declaration.id.span)
            }
            // `type HttpStatus = common.HttpStatus` names the framework's own type.
            AstKind::TSTypeAliasDeclaration(alias) => match &alias.type_annotation {
                TSType::TSTypeReference(reference) if reference.type_arguments.is_none() => return,
                TSType::TSImportType(_) => return,
                _ => (alias.span.start, alias.id.span),
            },
            AstKind::Class(class) => match &class.id {
                Some(id) => (
                    start_after_decorators(file, class.span, &class.decorators),
                    id.span,
                ),
                None => return,
            },
            AstKind::VariableDeclarator(declarator) => match &declarator.id {
                BindingPattern::BindingIdentifier(id)
                    if declarator
                        .init
                        .as_ref()
                        .is_some_and(|init| is_object(file, init)) =>
                {
                    (
                        file.semantic.nodes().parent_node(node.id()).span().start,
                        id.span,
                    )
                }
                _ => return,
            },
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
    if matches!(
        value.get_inner_expression(),
        Expression::ObjectExpression(_)
    ) {
        return true;
    }
    call_of(value).is_some_and(|call| {
        global_member(file, &call.callee) == Some(("Object", "freeze"))
            && call
                .arguments
                .first()
                .and_then(|argument| argument.as_expression())
                .is_some_and(|argument| is_object(file, argument))
    })
}
