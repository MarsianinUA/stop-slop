use std::collections::BTreeSet;

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
        let mut found: BTreeSet<(u32, u32, &'static str)> = BTreeSet::new();
        for node in cx.file().semantic.nodes().iter() {
            let mark = match node.kind() {
                AstKind::StringLiteral(string) => text_mark(string.value.as_str()),
                AstKind::TemplateLiteral(template) => {
                    let whole = template.expressions.is_empty();
                    template.quasis.iter().find_map(|quasi| {
                        let value = &quasi.value;
                        let text = value.cooked.as_ref().unwrap_or(&value.raw).as_str();
                        if whole {
                            text_mark(text)
                        } else {
                            part_mark(text)
                        }
                    })
                }
                AstKind::IdentifierName(name) => identifier_mark(name.name.as_str()),
                AstKind::IdentifierReference(name) => identifier_mark(name.name.as_str()),
                AstKind::BindingIdentifier(name) => identifier_mark(name.name.as_str()),
                _ => None,
            };
            if let Some(mark) = mark {
                let span = node.span();
                found.insert((span.start, span.end, mark));
            }
        }
        for (start, end, mark) in found {
            cx.report(Span::new(start, end)).arg("mark", mark);
        }
    }
}

fn text_mark(text: &str) -> Option<&'static str> {
    if text == "/stand" {
        return Some("/stand");
    }
    part_mark(text)
}

/// For a part of a template, which `${}` continues: `/stand${suffix}` is not the whole text.
fn part_mark(text: &str) -> Option<&'static str> {
    NAMES.into_iter().find(|name| text.contains(*name))
}

fn identifier_mark(name: &str) -> Option<&'static str> {
    name.contains("STOP_SLOP").then_some("STOP_SLOP")
}
