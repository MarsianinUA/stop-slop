use kit::oxc::ast::ast::Statement;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// Words of an entry file's name: `main.ts`, `bootstrap-env.ts`.
    entries: Vec<String>,
    /// Folders of command-line scripts: `scripts/seed.ts`.
    dirs: Vec<String>,
}

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let body = &file.semantic.nodes().program().body;
        if !exports_something(body)
            || file_name_has_word(file, &self.entries)
            || is_cli_script(file, &self.dirs)
        {
            return;
        }
        report_load_time_work(body, cx);
    }
}

/// The module gives other files something to import. One without exports can only be imported
/// for its effect (`import './telemetry'`), so it is a bootstrap module by design; `export {}`
/// only marks a file as a module.
fn exports_something(body: &[Statement<'_>]) -> bool {
    body.iter().any(|statement| match statement {
        Statement::ImportDeclaration(_) => false,
        Statement::ExportNamedDeclaration(export) => !export.specifiers.is_empty(),
        statement => statement.is_module_declaration(),
    })
}

/// The statements that do work when the module loads: anything but a declaration, an import or an
/// export. The body of a bare block runs too (`no-bare-block` reports the braces).
fn report_load_time_work(statements: &[Statement<'_>], cx: &mut Cx<'_, '_>) {
    for statement in statements {
        match statement {
            Statement::BlockStatement(block) => report_load_time_work(&block.body, cx),
            Statement::ExpressionStatement(_)
            | Statement::IfStatement(_)
            | Statement::SwitchStatement(_)
            | Statement::ForStatement(_)
            | Statement::ForInStatement(_)
            | Statement::ForOfStatement(_)
            | Statement::WhileStatement(_)
            | Statement::DoWhileStatement(_)
            | Statement::TryStatement(_)
            | Statement::LabeledStatement(_)
            | Statement::ThrowStatement(_) => {
                cx.report(statement.span());
            }
            _ => {}
        }
    }
}
