use kit::oxc::ast::ast::Expression;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::NewExpression, AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let is_proxy = match node.kind() {
            AstKind::NewExpression(new) => is_global_proxy(file, &new.callee),
            AstKind::CallExpression(call) => matches!(
                call.callee.get_inner_expression(),
                Expression::StaticMemberExpression(member)
                    if member.property.name == "revocable" && is_global_proxy(file, &member.object)
            ),
            _ => false,
        };
        if is_proxy {
            cx.report(node.span());
        }
    }
}

/// An imported or local `Proxy` is someone's class, not the built-in.
fn is_global_proxy(file: &File<'_>, expression: &Expression<'_>) -> bool {
    matches!(expression.get_inner_expression(), Expression::Identifier(id)
        if id.name == "Proxy" && file.semantic.is_reference_to_global_variable(id))
}
