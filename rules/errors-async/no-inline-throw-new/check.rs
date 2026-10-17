use kit::oxc::ast::ast::Expression;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// A folder of error factories: `src/orders/errors/order.errors.ts`.
    dirs: Vec<String>,
}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::ThrowStatement];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let AstKind::ThrowStatement(statement) = node.kind() else {
            return;
        };
        let Expression::NewExpression(new) = &statement.argument else {
            return;
        };
        let in_factories = file
            .path
            .parent()
            .is_some_and(|dirs| dirs.iter().any(|dir| self.dirs.iter().any(|d| d == dir)));
        if !in_factories {
            cx.report(statement.span)
                .arg("class", new.callee.span().source_text(file.text));
        }
    }
}
