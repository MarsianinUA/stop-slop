use kit::oxc::semantic::NodeId;
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
        if is_one_letter(name) {
            // With its `#`, as the reader sees it.
            cx.report(span).arg("name", span.source_text(file.text));
        }
    }

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let scoping = file.semantic.scoping();
        let nodes = file.semantic.nodes();
        // Built on the first name that needs it: most files have no single-letter name.
        let mut shorthands = None;
        for symbol in scoping.symbol_ids() {
            let name = scoping.symbol_name(symbol);
            if !is_one_letter(name) || !is_own_name(file, symbol) {
                continue;
            }
            let declaration = scoping.symbol_declaration(symbol);
            let is_allowed = match nodes.kind(declaration) {
                // A catch parameter is `catch-param-name`'s; `T` and `K` are a TypeScript custom.
                AstKind::CatchParameter(_)
                | AstKind::TSTypeParameter(_)
                | AstKind::TSMappedType(_) => true,
                AstKind::FormalParameter(_) | AstKind::FormalParameterRest(_) => {
                    is_short_callback(file, nodes.parent_id(nodes.parent_id(declaration)))
                }
                // A `for..of` or `for..in` variable is not a counter: it holds a value.
                AstKind::VariableDeclarator(_) => matches!(
                    nodes.parent_kind(nodes.parent_id(declaration)),
                    AstKind::ForStatement(_)
                ),
                _ => false,
            };
            if is_allowed {
                continue;
            }
            let span = scoping.symbol_span(symbol);
            let shorthands = shorthands.get_or_insert_with(|| shorthand_starts(file));
            if !shorthands.contains(&span.start) {
                cx.report(span).arg("name", name);
            }
        }
    }
}

/// `_` and `$` are not letters.
fn is_one_letter(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(char::is_alphabetic) && chars.next().is_none()
}

/// A callback on one line, or an arrow with an expression body: a formatter wraps a long one
/// (`sort((a, b) =>\n  a.x - b.x)`), not its author.
fn is_short_callback(file: &File<'_>, function: NodeId) -> bool {
    let nodes = file.semantic.nodes();
    let function = nodes.get_node(function);
    let is_expression_body = match function.kind() {
        AstKind::ArrowFunctionExpression(arrow) => arrow.is_expression(),
        AstKind::Function(value) if value.is_expression() => false,
        _ => return false,
    };
    is_callback(file, function)
        && (is_expression_body || !function.span().source_text(file.text).contains('\n'))
}

/// Where each `{ a }` or `{ a = 1 }` binding starts: the key gives the name.
fn shorthand_starts(file: &File<'_>) -> Vec<u32> {
    file.semantic
        .nodes()
        .iter()
        .filter_map(|node| match node.kind() {
            AstKind::BindingProperty(property) if property.shorthand => {
                Some(property.value.span().start)
            }
            _ => None,
        })
        .collect()
}
