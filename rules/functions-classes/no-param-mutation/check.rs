use kit::oxc::ast::ast::{BindingIdentifier, BindingPattern, Expression, FormalParameter, TSType};
use kit::oxc::semantic::NodeId;
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
    const NODES: &'static [AstType] = &[AstType::Function, AstType::ArrowFunctionExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let params = match node.kind() {
            AstKind::Function(function) if function.body.is_some() => &function.params,
            AstKind::ArrowFunctionExpression(arrow) => &arrow.params,
            _ => return,
        };
        if params.items.is_empty() || is_ambient(file, node.id()) {
            return;
        }
        let scoping = file.semantic.scoping();
        let skip_first = is_reduce_callback(file, node);
        for (index, parameter) in params.items.iter().enumerate() {
            // A parameter property is a field of the instance.
            if parameter.has_modifier() || index == 0 && skip_first {
                continue;
            }
            let collection = is_collection(file, parameter);
            let mut ids = Vec::new();
            bindings(&parameter.pattern, &mut ids);
            for id in ids {
                let Some(symbol) = id.symbol_id.get() else {
                    continue;
                };
                // One finding per bound name, at its first write: the agent decides once for it.
                let first = scoping
                    .get_resolved_references(symbol)
                    .filter_map(|reference| {
                        let id = reference.node_id();
                        member_write(file, id)
                            .or_else(|| assign_target(file, id))
                            .or_else(|| mutator_call(file, id).filter(|_| collection))
                    })
                    .min_by_key(|span| span.start);
                if let Some(span) = first {
                    cx.report(span).arg("name", id.name);
                }
            }
        }
    }
}

/// The names a parameter binds, without `...rest`: a rest element is a new array or object.
fn bindings<'a>(pattern: &'a BindingPattern<'a>, out: &mut Vec<&'a BindingIdentifier<'a>>) {
    match pattern {
        BindingPattern::BindingIdentifier(id) => out.push(id),
        BindingPattern::ObjectPattern(object) => {
            for property in &object.properties {
                bindings(&property.value, out);
            }
        }
        BindingPattern::ArrayPattern(array) => {
            for element in array.elements.iter().flatten() {
                bindings(element, out);
            }
        }
        BindingPattern::AssignmentPattern(assignment) => bindings(&assignment.left, out),
    }
}

/// The function is the first argument of `.reduce(..)` / `.reduceRight(..)` with an initial value:
/// its first parameter is the accumulator, which the function may fill. Without an initial value
/// the first parameter starts as the caller's first element.
fn is_reduce_callback(file: &File<'_>, function: &AstNode<'_>) -> bool {
    let AstKind::CallExpression(call) = file.semantic.nodes().parent_kind(function.id()) else {
        return false;
    };
    call.arguments.len() >= 2
        && call
            .arguments
            .first()
            .is_some_and(|argument| argument.span() == function.span())
        && matches!(
            call.callee
                .get_member_expr()
                .and_then(|member| member.static_property_name()),
            Some("reduce" | "reduceRight")
        )
}

/// The parameter's own annotation is an array, a tuple or a lib `Array`, `Map`, `Set`, `WeakMap`
/// or `WeakSet`, also with `null` / `undefined` (`string[] | undefined`). Another type's `add` or `set` (`queue.add(job)`) may not change anything.
fn is_collection(file: &File<'_>, parameter: &FormalParameter<'_>) -> bool {
    let BindingPattern::BindingIdentifier(_) = parameter.pattern else {
        return false;
    };
    let Some(annotation) = &parameter.type_annotation else {
        return false;
    };
    is_nullable(&annotation.type_annotation, |ty| {
        let ty = unwrapped_type(file, ty);
        matches!(ty, TSType::TSArrayType(_) | TSType::TSTupleType(_))
            || ["Array", "Map", "Set", "WeakMap", "WeakSet"]
                .into_iter()
                .any(|name| lib_type(file, ty, name).is_some())
    })
}

/// The call when the reference is the target of the global `Object.assign(x, ..)`.
fn assign_target(file: &File<'_>, id: NodeId) -> Option<Span> {
    let nodes = file.semantic.nodes();
    let target = outer_assertion(file, nodes.get_node(id));
    let AstKind::CallExpression(call) = nodes.parent_kind(target.id()) else {
        return None;
    };
    if !call
        .arguments
        .first()
        .is_some_and(|argument| argument.span() == target.span())
    {
        return None;
    }
    matches!(call.callee.get_inner_expression(),
        Expression::StaticMemberExpression(member)
            if member.property.name == "assign" && is_global(file, &member.object, "Object"))
    .then_some(call.span)
}

/// The call when the reference is the receiver of a mutator: `items.push(..)`, `seen.add(..)`.
fn mutator_call(file: &File<'_>, id: NodeId) -> Option<Span> {
    let nodes = file.semantic.nodes();
    let receiver = outer_assertion(file, nodes.get_node(id));
    let AstKind::StaticMemberExpression(member) = nodes.parent_kind(receiver.id()) else {
        return None;
    };
    match nodes.parent_kind(nodes.parent_id(receiver.id())) {
        AstKind::CallExpression(call)
            if call.callee.span() == member.span
                && MUTATORS.contains(&member.property.name.as_str()) =>
        {
            Some(call.span)
        }
        _ => None,
    }
}
