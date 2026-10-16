use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = PRIVATE_MEMBER_NODES;

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let Some((name, span)) = private_member(file, node) else {
            return;
        };
        if let Some(suggestion) = unprefixed(name) {
            // With its `#`, as the reader sees it.
            let shown = span.source_text(file.text);
            cx.report(span)
                .arg("name", shown)
                .arg("suggestion", shown.replacen(name, suggestion, 1));
        }
    }

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let scoping = file.semantic.scoping();
        for symbol in scoping.symbol_ids() {
            let name = scoping.symbol_name(symbol);
            let Some(suggestion) = unprefixed(name) else {
                continue;
            };
            // On a symbol nothing reads, `_` marks it unused on purpose for `noUnusedParameters`
            // and ESLint's `argsIgnorePattern`: `(_request, response) => ..`.
            let unused =
                scoping.get_resolved_reference_ids(symbol).is_empty() && !is_exported(file, symbol);
            if unused || !is_own_name(file, symbol) {
                continue;
            }
            cx.report(scoping.symbol_span(symbol))
                .arg("name", name)
                .arg("suggestion", suggestion)
                .fix(rename(file, symbol, suggestion).unwrap_or_default());
        }
    }
}

/// `name` without its `_`; `None` for `_` itself and for a `__` start, which copies the
/// platform's own names (`__dirname`, `__DEV__`).
fn unprefixed(name: &str) -> Option<&str> {
    name.strip_prefix('_')
        .filter(|rest| !rest.is_empty() && !rest.starts_with('_'))
}
