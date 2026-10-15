use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::TSInterfaceDeclaration];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::TSInterfaceDeclaration(interface) = node.kind() else {
            return;
        };
        let name = interface.id.name.as_str();
        let Some(suggestion) = name.strip_prefix('I') else {
            return;
        };
        // `IPAddress`, `IOStream` and `I18nConfig` start with a word of their own; so does
        // `IPv4Address`, which `words` splits into `I`, `Pv4`.
        let ip_version = suggestion
            .strip_prefix("Pv")
            .is_some_and(|rest| rest.starts_with(|ch: char| ch.is_ascii_digit()));
        if ip_version || !matches!(words(name).as_slice(), ["I", _, ..]) {
            return;
        }
        let Some(symbol) = interface.id.symbol_id.get() else {
            return;
        };
        let file = cx.file();
        // A merged interface is reported once, at its first declaration.
        let first = file.semantic.scoping().symbol_declaration(symbol);
        if first != node.id() || !is_own_name(file, symbol) {
            return;
        }
        cx.report(interface.id.span)
            .arg("name", name)
            .arg("suggestion", suggestion)
            .fix(rename(file, symbol, suggestion).unwrap_or_default());
    }
}
