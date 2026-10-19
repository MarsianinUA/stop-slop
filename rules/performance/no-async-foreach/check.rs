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
        let is_async = match call
            .arguments
            .first()
            .and_then(Argument::as_expression)
            .map(Expression::get_inner_expression)
        {
            Some(Expression::ArrowFunctionExpression(arrow)) => arrow.r#async,
            Some(Expression::FunctionExpression(function)) => function.r#async,
            _ => false,
        };
        if member.static_property_name() != Some("forEach") || !is_async {
            return;
        }
        let file = cx.file();
        cx.report(Span::new(call.span.start, member.span().end))
            .arg(
                "items",
                one_line(member.object().span().source_text(file.text)),
            );
    }
}
