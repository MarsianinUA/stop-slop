use std::collections::BTreeSet;

use kit::oxc::ast::ast::TemplateElement;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

/// What a string or the text of a template must not hold. `/stand` alone counts only as the
/// whole text: `/standard` is not the stand.
const NAMES: [&str; 5] = [
    "/stand/",
    "STOP_SLOP",
    "stand-guest",
    "microsandbox",
    "preload.cjs",
];

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        // A shorthand (`{ STOP_SLOP_RUN }`, an import without `as`) is two identifiers on one
        // span, which get one finding.
        let mut found: BTreeSet<(Span, &'static str)> = BTreeSet::new();
        for node in cx.file().semantic.nodes().iter() {
            let mark = match node.kind() {
                AstKind::StringLiteral(string) => text_mark(string.value.as_str(), true),
                AstKind::TemplateLiteral(template) => {
                    template_mark(&template.quasis, template.expressions.is_empty())
                }
                AstKind::TSTemplateLiteralType(template) => {
                    template_mark(&template.quasis, template.types.is_empty())
                }
                AstKind::PrivateIdentifier(name) => identifier_mark(name.name.as_str()),
                kind => kind
                    .identifier_name()
                    .and_then(|name| identifier_mark(name.as_str())),
            };
            if let Some(mark) = mark {
                found.insert((node.span(), mark));
            }
        }
        for (span, mark) in found {
            cx.report(span).arg("mark", mark);
        }
    }
}

/// `whole` is false for a template with `${}`, whose parts are never the whole text:
/// `/stand${suffix}` is not `/stand`.
fn template_mark(quasis: &[TemplateElement<'_>], whole: bool) -> Option<&'static str> {
    quasis.iter().find_map(|quasi| {
        let value = &quasi.value;
        text_mark(value.cooked.as_ref().unwrap_or(&value.raw).as_str(), whole)
    })
}

fn text_mark(text: &str, whole: bool) -> Option<&'static str> {
    if whole && text == "/stand" {
        return Some("/stand");
    }
    NAMES.into_iter().find(|name| text.contains(*name))
}

fn identifier_mark(name: &str) -> Option<&'static str> {
    name.contains("STOP_SLOP").then_some("STOP_SLOP")
}
