use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::BlockStatement];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let bare = match file.semantic.nodes().parent_kind(node.id()) {
            AstKind::Program(_)
            | AstKind::FunctionBody(_)
            | AstKind::BlockStatement(_)
            | AstKind::StaticBlock(_)
            | AstKind::TSModuleBlock(_) => true,
            // `case 'paid': { const total = .. }` scopes the names of one case.
            AstKind::SwitchCase(case) => case
                .consequent
                .first()
                .is_none_or(|first| first.span() != node.span()),
            _ => false,
        };
        if bare {
            cx.report(node.span());
        }
    }
}
