use kit::oxc::ast::ast::Statement;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        if file.path.file_stem() != Some("index") {
            return;
        }
        let body = &file.semantic.nodes().program().body;
        let Some(first) = body.first() else {
            return;
        };
        // `export * as ns from` names what it exports, so it is not a catch-all.
        if body.iter().all(|statement| {
            matches!(statement, Statement::ExportAllDeclaration(export) if export.exported.is_none())
        }) {
            cx.report(first.span()).arg("count", body.len());
        }
    }
}
