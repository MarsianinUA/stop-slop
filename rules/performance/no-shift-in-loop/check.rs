use kit::oxc::ast::ast::Expression;
use kit::oxc::semantic::SymbolId;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::CallExpression(call) = node.kind() else {
            return;
        };
        let Some(member) = call.callee.get_member_expr() else {
            return;
        };
        let Some(method @ ("shift" | "unshift")) = member.static_property_name() else {
            return;
        };
        let Expression::Identifier(array) = member.object().get_inner_expression() else {
            return;
        };
        let file = cx.file();
        let reference = file.semantic.scoping().get_reference(array.reference_id());
        let Some(symbol) = reference.symbol_id() else {
            return;
        };
        // The deque of a library shifts in constant time: only an array the syntax shows counts.
        if !is_known_array(file, member.object()) {
            return;
        }
        let Some(around) = loop_around(file, node.id()) else {
            return;
        };
        if goes_through(file, &around, symbol) {
            cx.report(call.span)
                .arg("array", array.name.as_str())
                .arg("method", method)
                .arg("loop", around.label(file));
        }
    }
}

/// The loop goes through the array: it is the loop's collection, or the test of `while`, `do` or
/// `for` reads it (`while (queue.length)`, `do .. while (stack.length < limit)`).
fn goes_through(file: &File<'_>, around: &LoopAround<'_>, symbol: SymbolId) -> bool {
    let scoping = file.semantic.scoping();
    let is_array = |expression: &Expression<'_>| {
        matches!(expression.get_inner_expression(), Expression::Identifier(id)
            if scoping.get_reference(id.reference_id()).symbol_id() == Some(symbol))
    };
    if around.over.is_some_and(is_array) {
        return true;
    }
    let test = match around.node.kind() {
        AstKind::WhileStatement(statement) => Some(&statement.test),
        AstKind::DoWhileStatement(statement) => Some(&statement.test),
        AstKind::ForStatement(statement) => statement.test.as_ref(),
        _ => None,
    };
    let nodes = file.semantic.nodes();
    test.is_some_and(|test| {
        scoping.get_resolved_references(symbol).any(|reference| {
            test.span()
                .contains_inclusive(nodes.get_node(reference.node_id()).span())
        })
    })
}
