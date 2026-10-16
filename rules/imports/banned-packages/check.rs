use std::collections::BTreeMap;

use kit::oxc::ast::ast::{Argument, Expression, StringLiteral};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// A package and what the platform offers instead.
    packages: BTreeMap<String, String>,
}

impl Rule for Check {
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
        if module.starts_with('.') || module.starts_with('/') || module.starts_with("node:") {
            return;
        }
        let package = package_name(module);
        let Some(instead) = self.packages.get(package) else {
            return;
        };
        cx.report(node.span())
            .arg("package", package)
            .arg("instead", instead);
    }
}

/// Same as `module_source` in `node-protocol`.
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

/// Same as `package_name` in `kit::import_kind`: `@scope/name/sub` -> `@scope/name`,
/// `name/sub` -> `name`.
fn package_name(source: &str) -> &str {
    let end = if source.starts_with('@') {
        source.match_indices('/').nth(1)
    } else {
        source.match_indices('/').next()
    };
    end.map_or(source, |(at, _)| &source[..at])
}
