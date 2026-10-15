use kit::oxc::ast::ast::{Argument, Expression, StringLiteral};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const USES: &'static [Fact] = &[Fact::Packages];
    const NODES: &'static [AstType] = &[
        AstType::ImportDeclaration,
        AstType::ExportFromDeclaration,
        AstType::ExportAllDeclaration,
        AstType::ImportExpression,
        AstType::TSImportType,
        AstType::TSExternalModuleReference,
        AstType::CallExpression,
    ];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let Some(source) = module_source(node.kind(), file) else {
            return;
        };
        let module = source.value.as_str();
        if module.starts_with("node:") || import_kind(file, cx.facts(), module) != ImportKind::Node
        {
            return;
        }
        cx.report(source.span).arg("module", module).fix([Edit {
            span: Span::empty(source.span.start + 1),
            text: "node:".into(),
        }]);
    }
}

/// The literal that names the module; its span takes the quotes too.
fn module_source<'a>(kind: AstKind<'a>, file: &File<'a>) -> Option<&'a StringLiteral<'a>> {
    match kind {
        AstKind::ImportDeclaration(import) => Some(&import.source),
        AstKind::ExportFromDeclaration(export) => Some(&export.source),
        AstKind::ExportAllDeclaration(export) => Some(&export.source),
        AstKind::ImportExpression(import) => match &import.source {
            Expression::StringLiteral(source) => Some(source),
            _ => None,
        },
        AstKind::TSImportType(import) => Some(&import.source),
        AstKind::TSExternalModuleReference(reference) => Some(&reference.expression),
        AstKind::CallExpression(call) => match (&call.callee, call.arguments.as_slice()) {
            (Expression::Identifier(callee), [Argument::StringLiteral(source)])
                if callee.name == "require"
                    && file.semantic.is_reference_to_global_variable(callee) =>
            {
                Some(source)
            }
            _ => None,
        },
        _ => None,
    }
}
