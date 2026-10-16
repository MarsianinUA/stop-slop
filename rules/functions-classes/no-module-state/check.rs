use kit::oxc::ast::ast::{Expression, UnaryOperator};
use kit::oxc::semantic::{NodeId, ScopeFlags};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

/// Methods that change a `Map`, `Set`, `WeakMap`, `WeakSet` or array in place: the language's
/// own, not a taste to configure.
const MUTATORS: &[&str] = &[
    "set",
    "delete",
    "clear",
    "add",
    "push",
    "pop",
    "shift",
    "unshift",
    "splice",
    "sort",
    "reverse",
    "fill",
    "copyWithin",
];

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let scoping = file.semantic.scoping();
        for binding in top_bindings(file) {
            let is_state = match binding.kind {
                TopKind::Variable => true,
                TopKind::Constant => is_changed(file, &binding),
                TopKind::Function => false,
            };
            if !is_state {
                continue;
            }
            for &symbol in &binding.symbols {
                cx.report(scoping.symbol_span(symbol))
                    .arg("name", scoping.symbol_name(symbol));
            }
        }
    }
}

fn is_changed(file: &File<'_>, binding: &TopBinding<'_>) -> bool {
    let [symbol] = binding.symbols[..] else {
        return false;
    };
    let container = binding.init.is_some_and(|init| is_container(file, init));
    file.semantic
        .scoping()
        .get_resolved_references(symbol)
        .any(|reference| {
            let id = reference.node_id();
            (is_member_write(file, id) || container && is_mutator_call(file, id))
                && at_run_time(file, id)
        })
}

/// A write at the top of the module fills the value while it loads (`ROUTES.set(..)` under the
/// declaration); one inside a function, a static block or a field's value is code changing it.
fn at_run_time(file: &File<'_>, id: NodeId) -> bool {
    let scoping = file.semantic.scoping();
    let nodes = file.semantic.nodes();
    let node = nodes.get_node(id);
    let span = node.span();
    scoping
        .scope_ancestors(node.scope_id())
        .any(|scope| {
            scoping
                .scope_flags(scope)
                .intersects(ScopeFlags::Function | ScopeFlags::ClassStaticBlock)
        })
        || nodes.ancestor_kinds(id).any(|kind| {
            matches!(kind, AstKind::PropertyDefinition(property)
                if property.value.as_ref().is_some_and(|value| value.span().contains_inclusive(span)))
        })
}

/// The reference is the root of a member chain that is assigned, updated or deleted: `x.a = ..`,
/// `x[k] += ..`, `x.a.b++`, `delete x.a`.
fn is_member_write(file: &File<'_>, id: NodeId) -> bool {
    let nodes = file.semantic.nodes();
    let mut current = outer_assertion(file, nodes.get_node(id));
    let mut is_member = false;
    loop {
        let parent = nodes.parent_node(current.id());
        let span = current.span();
        if let Some(member) = parent.kind().as_member_expression_kind() {
            // The key of `o[x]` is not the object that changes.
            if member.object().span() != span {
                return false;
            }
            is_member = true;
            current = outer_assertion(file, parent);
            continue;
        }
        return is_member
            && match parent.kind() {
                AstKind::AssignmentExpression(assignment) => assignment.left.span() == span,
                AstKind::UpdateExpression(_) => true,
                AstKind::UnaryExpression(unary) => unary.operator == UnaryOperator::Delete,
                _ => false,
            };
    }
}

/// `cache.set(..)`, `queue.push(..)`: the reference is the receiver of a mutator call.
fn is_mutator_call(file: &File<'_>, id: NodeId) -> bool {
    let nodes = file.semantic.nodes();
    let receiver = outer_assertion(file, nodes.get_node(id));
    let AstKind::StaticMemberExpression(member) = nodes.parent_kind(receiver.id()) else {
        return false;
    };
    MUTATORS.contains(&member.property.name.as_str())
        && matches!(
            nodes.parent_kind(nodes.parent_id(receiver.id())),
            AstKind::CallExpression(call) if call.callee.span() == member.span
        )
}

/// A new instance of a global collection class or an array literal: a value made to be filled.
/// A plain object has no methods that change it in place: its `set` or `add` is the file's own.
fn is_container(file: &File<'_>, init: &Expression<'_>) -> bool {
    match init.get_inner_expression() {
        Expression::NewExpression(new) => ["Map", "Set", "WeakMap", "WeakSet", "Array"]
            .iter()
            .any(|name| is_global(file, new.callee.get_inner_expression(), name)),
        Expression::ArrayExpression(_) => true,
        _ => false,
    }
}
