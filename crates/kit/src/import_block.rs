use oxc_ast::ast::{ImportDeclaration, Statement};

use crate::{Facts, File, ImportKind, has_blank_line, import_kind};

/// The place of a module in the order of `import-order`: Node built-ins 0, external packages 1,
/// workspace packages and aliases 2, relative paths 3.
pub fn import_group(file: &File<'_>, facts: &dyn Facts, source: &str) -> u8 {
    match import_kind(file, facts, source) {
        ImportKind::Node => 0,
        ImportKind::External => 1,
        ImportKind::Workspace | ImportKind::Alias => 2,
        ImportKind::Relative => 3,
    }
}

/// The blocks `import-order` sorts: runs of top-level imports with bindings. A side-effect import
/// (`import './telemetry'`, `import {} from 'x'`), `import defer` / `import source` and any other
/// statement end a block, and so do the value imports on the lines right under a side-effect
/// import, while they stay in one group: `import { metrics } from './metrics'` right under
/// `import './telemetry'` loads in the order written on purpose.
pub fn import_blocks<'a>(
    file: &File<'a>,
    facts: &dyn Facts,
) -> Vec<Vec<&'a ImportDeclaration<'a>>> {
    // The end of the side-effect import or of the load-order import just above, and the group of
    // a load-order import: none right under the side-effect import.
    let mut loading: Option<(u32, Option<u8>)> = None;
    let sorted: Vec<Option<&ImportDeclaration<'a>>> = file
        .semantic
        .nodes()
        .program()
        .body
        .iter()
        .map(|statement| {
            let Statement::ImportDeclaration(import) = statement else {
                loading = None;
                return None;
            };
            let has_bindings = import
                .specifiers
                .as_ref()
                .is_some_and(|list| !list.is_empty());
            if import.phase.is_some() || !has_bindings {
                loading = import.phase.is_none().then_some((import.span.end, None));
                return None;
            }
            let load_order = loading.and_then(|(end, group)| {
                let own = import_group(file, facts, &import.source.value);
                let joins = match group {
                    None => import.import_kind.is_value(),
                    Some(group) => group == own,
                };
                (joins && !has_blank_line(&file.text[end as usize..import.span.start as usize]))
                    .then_some(own)
            });
            loading = load_order.map(|group| (import.span.end, Some(group)));
            load_order.is_none().then_some(&**import)
        })
        .collect();
    sorted
        .chunk_by(|a, b| a.is_some() && b.is_some())
        .filter_map(|run| run.iter().copied().collect())
        .collect()
}
