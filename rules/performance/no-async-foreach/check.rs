use kit::oxc::ast::ast::{Argument, Expression};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::CallExpression(call) = node.kind() else {
            return;
        };
        let Some(member) = call.callee.get_member_expr() else {
            return;
        };
        if member.static_property_name() != Some("forEach") {
            return;
        }
        let callback = match call
            .arguments
            .first()
            .and_then(Argument::as_expression)
            .map(Expression::get_inner_expression)
        {
            Some(Expression::ArrowFunctionExpression(arrow)) => arrow.node_id.get(),
            Some(Expression::FunctionExpression(function)) => function.node_id.get(),
            _ => return,
        };
        let file = cx.file();
        if !returns_promise(file, file.semantic.nodes().get_node(callback)) {
            return;
        }
        cx.report(Span::new(call.span.start, member.span().end))
            .arg(
                "items",
                one_line(member.object().span().source_text(file.text)),
            );
    }
}
