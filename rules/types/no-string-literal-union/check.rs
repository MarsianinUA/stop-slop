use kit::oxc::ast::ast::{TSLiteral, TSType};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::TSTypeAliasDeclaration];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let AstKind::TSTypeAliasDeclaration(alias) = node.kind() else {
            return;
        };
        if is_string_union(&alias.type_annotation) {
            cx.report(Span::new(
                alias.span.start,
                alias.type_annotation.span().end,
            ))
            .arg("name", alias.id.span.source_text(file.text));
        }
    }
}

/// Two or more string literals, and nothing else but `null` and `undefined`:
/// `'peek' | 'charge' | null`.
fn is_string_union(ty: &TSType<'_>) -> bool {
    let TSType::TSUnionType(union) = ty else {
        return false;
    };
    let mut strings = 0;
    for member in &union.types {
        match member {
            TSType::TSLiteralType(literal)
                if matches!(literal.literal, TSLiteral::StringLiteral(_)) =>
            {
                strings += 1;
            }
            TSType::TSNullKeyword(_) | TSType::TSUndefinedKeyword(_) => {}
            _ => return false,
        }
    }
    strings >= 2
}
