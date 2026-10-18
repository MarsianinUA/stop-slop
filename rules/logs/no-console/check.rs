use kit::oxc::ast::ast::UnaryOperator;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// A folder of command-line scripts, whose output is the console: `scripts/seed.ts`.
    dirs: Vec<String>,
}

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        if is_cli_script(file, &self.dirs) {
            return;
        }
        let nodes = file.semantic.nodes();
        for reference in global_references(file, "console") {
            let node = outer_assertion(file, reference);
            let parent = nodes.parent_node(node.id());
            // `typeof console` and `typeof console.warn` write nothing, in a type and in a value.
            if matches!(
                parent.kind(),
                AstKind::TSTypeQuery(_) | AstKind::TSQualifiedName(_)
            ) || in_typeof(file, node, parent)
            {
                continue;
            }
            let (span, used) = used_as(file, node, parent);
            cx.report(span)
                .arg("use", one_line(used.source_text(file.text)));
        }
    }
}

/// The place and what it uses: the whole call for `console.warn(..)` (its arguments tell one
/// call from another) with `console.warn`, the member for `const log = console.log`, and
/// `console` itself where the object goes on (`new Logger(console)`, `const { log } = console`).
fn used_as<'a>(file: &File<'a>, node: &AstNode<'a>, parent: &AstNode<'a>) -> (Span, Span) {
    if !is_object_of(node, parent) {
        return (node.span(), node.span());
    }
    let method = outer_assertion(file, parent);
    match file.semantic.nodes().parent_kind(method.id()) {
        AstKind::CallExpression(call) if call.callee.span() == method.span() => {
            (call.span, parent.span())
        }
        _ => (parent.span(), parent.span()),
    }
}

fn in_typeof<'a>(file: &File<'a>, node: &AstNode<'a>, parent: &AstNode<'a>) -> bool {
    let operand = if is_object_of(node, parent) {
        outer_assertion(file, parent)
    } else {
        node
    };
    matches!(file.semantic.nodes().parent_kind(operand.id()),
        AstKind::UnaryExpression(unary) if unary.operator == UnaryOperator::Typeof)
}

fn is_object_of(node: &AstNode<'_>, parent: &AstNode<'_>) -> bool {
    parent
        .kind()
        .as_member_expression_kind()
        .is_some_and(|member| member.object().span() == node.span())
}
