use std::collections::BTreeMap;

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
        let Some(source) = module_source(file, node.kind()) else {
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
