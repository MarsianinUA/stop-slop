use std::collections::BTreeMap;

use kit::oxc::ast::ast::Statement;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// A global and the module that exports it under the same name.
    globals: BTreeMap<String, String>,
}

const BROWSER: &str = " This file imports no Node module, so first check that it runs only in Node: in code a browser also loads, the global is the only one.";

impl Rule for Check {
    // `@types/node` among the packages; `add_import` places the new line by `import_kind`.
    const USES: &'static [Fact] = &[Fact::Packages, Fact::Aliases];

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let facts = cx.facts();
        // Without Node's types the global is the DOM's, and its module does not compile.
        if !facts
            .package(file.path)
            .is_some_and(|package| package.dependencies.contains("@types/node"))
        {
            return;
        }
        // A project with `@types/node` may still have code for the browser (Next.js, Vite).
        let in_node = imports_node(file, facts);
        let scoping = file.semantic.scoping();
        let nodes = file.semantic.nodes();
        // Unresolved: no declaration or import of the name in the file holds the reference.
        let unresolved = scoping.root_unresolved_references();
        for (name, module) in &self.globals {
            let Some(references) = unresolved.get(name.as_str()) else {
                continue;
            };
            let Some(span) = references
                .iter()
                .map(|&reference| {
                    nodes
                        .get_node(scoping.get_reference(reference).node_id())
                        .span()
                })
                .min_by_key(|span| span.start)
            else {
                continue;
            };
            let fix = if in_node {
                add_import(file, facts, module, &[name]).unwrap_or_default()
            } else {
                Vec::new()
            };
            cx.report(span)
                .arg("name", name)
                .arg("module", module)
                .arg("hint", if in_node { "" } else { BROWSER })
                .fix(fix);
        }
    }
}

/// A value import of a Node built-in: the file runs in Node.
fn imports_node(file: &File<'_>, facts: &dyn kit::Facts) -> bool {
    file.semantic
        .nodes()
        .program()
        .body
        .iter()
        .any(|statement| {
            matches!(statement, Statement::ImportDeclaration(import)
                if import.import_kind.is_value()
                    && import_kind(file, facts, &import.source.value) == ImportKind::Node)
        })
}
