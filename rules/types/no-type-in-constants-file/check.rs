use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[
        AstType::TSTypeAliasDeclaration,
        AstType::TSInterfaceDeclaration,
    ];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        if cx.file().role != Some(Role::Constants) {
            return;
        }
        let (start, id) = match node.kind() {
            AstKind::TSTypeAliasDeclaration(alias) => (alias.span.start, &alias.id),
            AstKind::TSInterfaceDeclaration(interface) => (interface.span.start, &interface.id),
            _ => return,
        };
        cx.report(Span::new(start, id.span.end))
            .arg("name", id.name.as_str());
    }
}
