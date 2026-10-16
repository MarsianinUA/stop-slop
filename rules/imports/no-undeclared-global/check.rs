use std::collections::BTreeMap;

use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// A global and the module that exports it under the same name.
    globals: BTreeMap<String, String>,
}

impl Rule for Check {
    // `add_import` places the new line by `import_kind`.
    const USES: &'static [Fact] = &[Fact::Packages, Fact::Aliases];

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let facts = cx.facts();
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
            cx.report(span)
                .arg("name", name)
                .arg("module", module)
                .fix(add_import(file, facts, module, &[name]).unwrap_or_default());
        }
    }
}
