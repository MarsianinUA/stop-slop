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
        if self.is_script(file) {
            return;
        }
        let nodes = file.semantic.nodes();
        for reference in global_references(file, "console") {
            let node = outer_assertion(file, reference);
            let parent = nodes.parent_node(node.id());
            // `typeof console` in a type writes nothing.
            if matches!(parent.kind(), AstKind::TSTypeQuery(_)) {
                continue;
            }
            let (span, used) = used_as(file, node, parent);
            // One line in the text output, also for a member split over lines.
            let used = used
                .source_text(file.text)
                .split_whitespace()
                .collect::<Vec<_>>();
            cx.report(span).arg("use", used.join(" "));
        }
    }
}

impl Check {
    /// A file in a script folder, or one that starts with `#!`: a command run from a shell.
    fn is_script(&self, file: &File<'_>) -> bool {
        file.semantic.nodes().program().hashbang.is_some()
            || file
                .path
                .parent()
                .is_some_and(|dirs| dirs.iter().any(|dir| self.dirs.iter().any(|d| d == dir)))
    }
}

/// The place and what it uses: the whole call for `console.warn(..)` (its arguments tell one
/// call from another) with `console.warn`, the member for `const log = console.log`, and
/// `console` itself where the object goes on (`new Logger(console)`, `const { log } = console`).
fn used_as<'a>(file: &File<'a>, node: &AstNode<'a>, parent: &AstNode<'a>) -> (Span, Span) {
    let Some(member) = parent.kind().as_member_expression_kind() else {
        return (node.span(), node.span());
    };
    if member.object().span() != node.span() {
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
