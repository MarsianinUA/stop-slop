use kit::oxc::ast::ast::{ChainElement, Expression};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::ExpressionStatement];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let AstKind::ExpressionStatement(statement) = node.kind() else {
            return;
        };
        // The statement drops the call's value: `await`, `return`, `void` and an assignment keep it.
        let call = match statement.expression.get_inner_expression() {
            Expression::CallExpression(call) => call,
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::CallExpression(call) => call,
                _ => return,
            },
            _ => return,
        };
        let nodes = file.semantic.nodes();
        // `(async () => { .. })();` runs a function written right there.
        let (callee, name) = match call.callee.get_inner_expression() {
            Expression::ArrowFunctionExpression(arrow) => {
                (Some(nodes.get_node(arrow.node_id.get())), "async () => ..")
            }
            Expression::FunctionExpression(function) => (
                Some(nodes.get_node(function.node_id.get())),
                "async function () {..}",
            ),
            callee => (
                local_callee(file, call),
                callee.span().source_text(file.text),
            ),
        };
        if callee.is_some_and(|callee| returns_promise(file, callee)) {
            cx.report(call.span).arg("name", name);
        }
    }
}
