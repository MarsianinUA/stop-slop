use kit::oxc::ast::ast::{BindingPattern, CatchClause};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::CatchParameter];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::CatchParameter(parameter) = node.kind() else {
            return;
        };
        let BindingPattern::BindingIdentifier(id) = &parameter.pattern else {
            return;
        };
        let Some(symbol) = id.symbol_id.get() else {
            return;
        };
        let name = id.name.as_str();
        let file = cx.file();
        // `retryError` in a nested catch names what failed, as the message advises.
        let names_failure = name
            .strip_suffix("Error")
            .is_some_and(|what| !what.is_empty());
        // On a parameter nothing reads, `_` marks it unused on purpose for ESLint's
        // `caughtErrorsIgnorePattern`, as in `no-underscore-prefix`: `catch (_error) {}`.
        let unused = name.starts_with('_')
            && file
                .semantic
                .scoping()
                .get_resolved_reference_ids(symbol)
                .is_empty();
        if name == "error" || names_failure || unused {
            return;
        }
        let clause = file.semantic.nodes().parent_node(node.id());
        let edits = if is_nested(file, clause) {
            None
        } else {
            rename(file, symbol, "error")
        };
        cx.report(id.span)
            .arg("name", name)
            .fix(edits.into_iter().flatten());
    }
}

/// The catch lies in another catch with a named parameter or holds one. Such a pair gets no fix:
/// which of the two keeps `error` and what the other one names is a choice for a person.
fn is_nested(file: &File<'_>, clause: &AstNode<'_>) -> bool {
    let nodes = file.semantic.nodes();
    nodes.ancestors(clause.id()).any(is_named_catch)
        || nodes.iter().any(|node| {
            is_named_catch(node)
                && node.id() != clause.id()
                && clause.span().contains_inclusive(node.span())
        })
}

fn is_named_catch(node: &AstNode<'_>) -> bool {
    matches!(node.kind(), AstKind::CatchClause(CatchClause { param: Some(parameter), .. })
        if matches!(parameter.pattern, BindingPattern::BindingIdentifier(_)))
}
