use kit::oxc::ast::ast::VariableDeclarationKind;
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
        // `for (let item of items)`: a fresh binding on every turn, with no initializer. A `let`
        // under `for..of` / `for..in` can only be its binding: a body of one statement cannot
        // be a declaration.
        let looped = match file.semantic.nodes().parent_kind(node.id()) {
            AstKind::ForOfStatement(statement) => Some(statement.right.span()),
            AstKind::ForInStatement(statement) => Some(statement.right.span()),
            _ => None,
        };
        let mut names = Vec::new();
        let mut narrowed = Vec::new();
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
                if scoping.symbol_is_mutated(symbol) || is_script_global(file, symbol) {
                    return;
                }
                if narrows {
                    narrowed.push(symbol);
                }
                names.push(id.name.as_str());
            }
        }
        let span = looped.map_or(declaration.span, |right| {
            Span::new(declaration.span.start, right.end)
        });
        let reported = cx.report(span).arg("name", names.join("`, `"));
        // `const` keeps `let mode = 'fast'` as `'fast'`, and `mode === 'slow'` would stop
        // compiling. The annotation does not count: `no-trivial-annotation` may remove it in the
        // same pass.
        let noticed = narrowed
            .into_iter()
            .any(|symbol| relies_on_wide_type(file, symbol));
        if !noticed {
            reported.fix([Edit {
                span: Span::sized(declaration.span.start, 3),
                text: "const".into(),
            }]);
        }
    }
}
