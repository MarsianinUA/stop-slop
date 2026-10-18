use kit::oxc::ast::ast::VariableDeclarationKind;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::VariableDeclaration];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let AstKind::VariableDeclaration(declaration) = node.kind() else {
            return;
        };
        // `declare global { var cache: Cache }` is how TypeScript declares a global variable.
        if declaration.kind != VariableDeclarationKind::Var || is_ambient(file, node.id()) {
            return;
        }
        let Some(last) = declaration.declarations.last() else {
            return;
        };
        // A destructuring pattern may span lines.
        let names: Vec<String> = declaration
            .declarations
            .iter()
            .map(|declarator| one_line(declarator.id.span().source_text(file.text)))
            .collect();
        cx.report(Span::new(declaration.span.start, last.id.span().end))
            .arg("name", names.join("`, `"));
    }
}
