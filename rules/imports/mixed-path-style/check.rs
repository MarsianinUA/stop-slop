use kit::oxc::ast::ast::{Statement, TSModuleReference};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    // Without `Packages` a workspace package that is also a key of `paths` reads as an alias.
    const USES: &'static [Fact] = &[Fact::Packages, Fact::Aliases];

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        if cx.facts().aliases(file.path).is_empty() {
            return;
        }
        let mut parent = Vec::new();
        let mut alias = Vec::new();
        for statement in &file.semantic.nodes().program().body {
            let source = match statement {
                Statement::ImportDeclaration(import) => &import.source,
                Statement::ExportFromDeclaration(export) => &export.source,
                Statement::ExportAllDeclaration(export) => &export.source,
                Statement::TSImportEqualsDeclaration(import) => match &import.module_reference {
                    TSModuleReference::ExternalModuleReference(reference) => &reference.expression,
                    _ => continue,
                },
                _ => continue,
            };
            let module = source.value.as_str();
            // `./` is not a style: it stays in the file's own folder either way.
            if module == ".." || module.starts_with("../") {
                parent.push((statement.span(), module));
            } else if import_kind(file, cx.facts(), module) == ImportKind::Alias {
                alias.push((statement.span(), module));
            }
        }
        if parent.is_empty() || alias.is_empty() {
            return;
        }
        let (minority, style, other) = if parent.len() <= alias.len() {
            (parent, "`../` paths", "the alias")
        } else {
            (alias, "the alias", "`../` paths")
        };
        let (span, module) = minority[0];
        cx.report(span)
            .arg("source", module)
            .arg("style", style)
            .arg("other", other)
            .arg("count", minority.len());
    }
}
