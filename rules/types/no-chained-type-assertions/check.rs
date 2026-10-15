use std::iter;

use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::TSAsExpression, AstType::TSTypeAssertion];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let Some(outer) = type_assertion(node.kind()) else {
            return;
        };
        // Only the outer link reports, so a chain of three is one finding.
        if type_assertion(file.semantic.nodes().parent_kind(node.id())).is_some()
            || expression_assertion(outer.0).is_none()
        {
            return;
        }
        let only_const = iter::successors(Some(outer), |(value, _)| expression_assertion(value))
            .all(|(_, ty)| is_const_assertion(ty));
        if !only_const && !has_safety_comment(file, node.span()) {
            cx.report(node.span());
        }
    }
}
