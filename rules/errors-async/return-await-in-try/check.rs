use kit::oxc::ast::ast::{ChainElement, Expression};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::ReturnStatement];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let AstKind::ReturnStatement(statement) = node.kind() else {
            return;
        };
        let call = match statement
            .argument
            .as_ref()
            .map(Expression::get_inner_expression)
        {
            Some(Expression::CallExpression(call)) => call,
            Some(Expression::ChainExpression(chain)) => match &chain.expression {
                ChainElement::CallExpression(call) => call,
                _ => return,
            },
            _ => return,
        };
        if !in_try_of_async(file, node) {
            return;
        }
        // A function of this file that gives no promise: `return this.toView(row)`.
        let is_plain =
            local_callee(file, call).is_some_and(|callee| !returns_promise(file, callee));
        if !is_plain {
            cx.report(statement.span);
        }
    }
}

/// The node sits in the block of a `try` of its own function, and that function is `async`:
/// only there can the return be awaited.
fn in_try_of_async(file: &File<'_>, node: &AstNode<'_>) -> bool {
    let mut in_try = false;
    for ancestor in file.semantic.nodes().ancestors(node.id()) {
        match ancestor.kind() {
            AstKind::TryStatement(statement) => {
                in_try = in_try || statement.block.span.contains_inclusive(node.span());
            }
            // An async generator's `return` already awaits its value.
            AstKind::Function(function) => {
                return in_try && function.r#async && !function.generator;
            }
            AstKind::ArrowFunctionExpression(arrow) => return in_try && arrow.r#async,
            _ => {}
        }
    }
    false
}
