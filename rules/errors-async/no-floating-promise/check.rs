use kit::oxc::ast::ast::{CallExpression, Expression};
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
        let nodes = file.semantic.nodes();
        let mut calls = Vec::new();
        dropped(&statement.expression, &mut calls);
        for call in calls {
            // `(async () => { .. })();` runs a function written right there.
            let (callee, name) = match call.callee.get_inner_expression() {
                Expression::ArrowFunctionExpression(arrow) => (
                    Some(nodes.get_node(arrow.node_id.get())),
                    format!("{}() => ..", if arrow.r#async { "async " } else { "" }),
                ),
                Expression::FunctionExpression(function) => (
                    Some(nodes.get_node(function.node_id.get())),
                    format!(
                        "{}function {}() {{..}}",
                        if function.r#async { "async " } else { "" },
                        function.id.as_ref().map_or("", |id| id.name.as_str())
                    ),
                ),
                callee => (
                    local_callee(file, call),
                    one_line(callee.span().source_text(file.text)),
                ),
            };
            if callee.is_some_and(|callee| returns_promise(file, callee)) {
                cx.report(call.span).arg("name", name);
            }
        }
    }
}

/// The calls whose value the statement drops: the statement itself, a branch of `&&`, `||`,
/// `??` and `?:`, each part of `a(), b()`. `await`, `return`, `void` and an assignment keep it.
fn dropped<'a>(expression: &'a Expression<'a>, calls: &mut Vec<&'a CallExpression<'a>>) {
    match expression.get_inner_expression() {
        Expression::LogicalExpression(logical) => dropped(&logical.right, calls),
        Expression::ConditionalExpression(conditional) => {
            dropped(&conditional.consequent, calls);
            dropped(&conditional.alternate, calls);
        }
        Expression::SequenceExpression(sequence) => {
            for part in &sequence.expressions {
                dropped(part, calls);
            }
        }
        expression => calls.extend(call_of(expression)),
    }
}
