use kit::oxc::ast::ast::{Expression, ForStatementLeft, VariableDeclarationKind};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::VariableDeclaration];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let AstKind::VariableDeclaration(declaration) = node.kind() else {
            return;
        };
        let scoping = file.semantic.scoping();
        // A direct `eval` may assign any name of its scopes while the code runs.
        if declaration.kind != VariableDeclarationKind::Let
            || scoping.root_scope_flags().contains_direct_eval()
        {
            return;
        }
        // `for (let item of items)`: a fresh binding on every turn, with no initializer.
        let looped = match file.semantic.nodes().parent_kind(node.id()) {
            AstKind::ForOfStatement(statement) => looped_over(&statement.left, &statement.right),
            AstKind::ForInStatement(statement) => looped_over(&statement.left, &statement.right),
            _ => None,
        };
        let mut names = Vec::new();
        let mut noticed = false;
        for declarator in &declaration.declarations {
            // `let total;` gets its value later, and `const` needs it here.
            if declarator.init.is_none() && looped.is_none() {
                return;
            }
            let narrows = declarator.init.as_ref().is_some_and(narrows_to_literal);
            for id in declarator.id.get_binding_identifiers() {
                let Some(symbol) = id.symbol_id.get() else {
                    return;
                };
                // A top-level `let` of a script is global: another script may assign it.
                if scoping
                    .get_resolved_references(symbol)
                    .any(|reference| reference.is_write())
                    || is_script_global(file, symbol)
                {
                    return;
                }
                // `const` keeps `let mode = 'fast'` as `'fast'`, and `mode === 'slow'` would
                // stop compiling. The annotation does not count: `no-trivial-annotation` may
                // remove it in the same pass.
                noticed |= narrows && relies_on_wide_type(file, symbol);
                names.push(id.name.as_str());
            }
        }
        let span = looped.map_or(declaration.span, |right| {
            Span::new(declaration.span.start, right.end)
        });
        let reported = cx.report(span).arg("name", names.join("`, `"));
        if !noticed {
            reported.fix([Edit {
                span: Span::sized(declaration.span.start, 3),
                text: "const".into(),
            }]);
        }
    }
}

/// The span of what the loop goes over, when the declaration is the loop's binding.
fn looped_over(left: &ForStatementLeft<'_>, right: &Expression<'_>) -> Option<Span> {
    matches!(left, ForStatementLeft::VariableDeclaration(_)).then(|| right.span())
}
