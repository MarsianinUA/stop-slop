use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::TSAsExpression, AstType::TSTypeAssertion];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let Some((_, ty)) = type_assertion(node.kind()) else {
            return;
        };
        if is_const_assertion(ty) {
            return;
        }
        // A chain is one finding, on its outer link: `input as unknown as User`.
        let parent = file.semantic.nodes().parent_kind(node.id());
        if type_assertion(parent).is_some_and(|(_, outer)| !is_const_assertion(outer)) {
            return;
        }
        if !has_safety_comment(file, node.span()) {
            cx.report(node.span());
        }
    }
}
