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
        // `Reflect['get'](..)` too; an imported or local `Reflect` is not the built-in.
        if global_member(file, &call.callee) == Some(("Reflect", "get")) {
            cx.report(call.span);
        }
    }
}
