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
        let Some(source) = module_source(file, node.kind()) else {
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
