use kit::oxc::ast::ast::TSEnumMemberName;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::TSEnumDeclaration];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::TSEnumDeclaration(declaration) = node.kind() else {
            return;
        };
        let Some(symbol) = declaration.id.symbol_id.get() else {
            return;
        };
        let file = cx.file();
        if !is_own_name(file, symbol) {
            return;
        }
        // A string key (`'content-type'`) is an outside spelling.
        for member in &declaration.body.members {
            let TSEnumMemberName::Identifier(id) = &member.id else {
                continue;
            };
            let name = id.name.as_str();
            // `Case::Pascal` lets runs of capitals through for acronyms, so `ETHEREUM` passes it.
            // A one-letter member (`Grade.A`) is all capitals too, but `apply` keeps it as it is.
            if Case::Pascal.matches(name) && !Case::Constant.matches(name) {
                continue;
            }
            let Some(suggestion) = Case::Pascal.apply(name).filter(|pascal| pascal != name) else {
                continue;
            };
            cx.report(id.span)
                .arg("name", name)
                .arg("suggestion", suggestion);
        }
    }
}
