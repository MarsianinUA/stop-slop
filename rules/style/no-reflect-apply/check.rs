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
        let file = cx.file();
        // `Reflect['apply'](..)` too; an imported or local `Reflect` is not the built-in.
        let found = call.callee.get_member_expr().is_some_and(|member| {
            member.static_property_name() == Some("apply")
                && is_global(file, member.object().get_inner_expression(), "Reflect")
        });
        if found {
            cx.report(call.span);
        }
    }
}
