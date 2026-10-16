use kit::oxc::ast::ast::Expression;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::NewExpression, AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        // An imported or local `Proxy` is someone's class, not the built-in.
        let is_proxy = |expression: &Expression<'_>| {
            is_global(file, expression.get_inner_expression(), "Proxy")
        };
        let found = match node.kind() {
            AstKind::NewExpression(new) => is_proxy(&new.callee),
            AstKind::CallExpression(call) => matches!(
                call.callee.get_inner_expression(),
                Expression::StaticMemberExpression(member)
                    if member.property.name == "revocable" && is_proxy(&member.object)
            ),
            _ => false,
        };
        if found {
            cx.report(node.span());
        }
    }
}
