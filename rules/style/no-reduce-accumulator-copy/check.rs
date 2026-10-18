use kit::oxc::ast::ast::{Argument, BindingPattern, Expression};
use kit::oxc::semantic::{NodeId, SymbolId};
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
        let is_reduce = call.callee.get_member_expr().is_some_and(|member| {
            matches!(
                member.static_property_name(),
                Some("reduce" | "reduceRight")
            )
        });
        if !is_reduce || call.arguments.len() > 2 {
            return;
        }
        let Some((callback, accumulator)) = call
            .arguments
            .first()
            .and_then(Argument::as_expression)
            .and_then(reducer)
        else {
            return;
        };
        let file = cx.file();
        // `concat` and `slice` copy an array but join a string: the initial value tells which.
        let starts_as_array = call
            .arguments
            .get(1)
            .and_then(Argument::as_expression)
            .is_some_and(|initial| is_known_array(file, initial));
        let scoping = file.semantic.scoping();
        let nodes = file.semantic.nodes();
        let mut names = vec![accumulator];
        let mut copies = Vec::new();
        while let Some(symbol) = names.pop() {
            for reference in scoping.get_resolved_references(symbol) {
                let node = nodes.get_node(reference.node_id());
                // A copy in a nested function does not run on every step.
                let owner = nodes
                    .ancestors(node.id())
                    .find(|ancestor| ancestor.kind().is_function_like());
                if owner.is_none_or(|function| function.id() != callback) {
                    continue;
                }
                let value = outer_assertion(file, node);
                if let Some(alias) = alias_of(file, value) {
                    names.push(alias);
                } else if let Some(copy) = copy_of(file, value, starts_as_array) {
                    copies.push(copy);
                }
            }
        }
        copies.sort_by_key(|span| span.start);
        copies.dedup();
        let name = scoping.symbol_name(accumulator);
        for copy in copies {
            cx.report(copy).arg("name", name);
        }
    }
}

/// The callback (an arrow or a function expression) and its first parameter, the accumulator.
fn reducer(callback: &Expression<'_>) -> Option<(NodeId, SymbolId)> {
    let (id, params) = match callback.get_inner_expression() {
        Expression::ArrowFunctionExpression(arrow) => (arrow.node_id.get(), &arrow.params),
        Expression::FunctionExpression(function) => (function.node_id.get(), &function.params),
        _ => return None,
    };
    let BindingPattern::BindingIdentifier(accumulator) = &params.items.first()?.pattern else {
        return None;
    };
    Some((id, accumulator.symbol_id.get()?))
}

/// `const alias = acc`: another name for the accumulator; a `let` may change what it holds.
fn alias_of(file: &File<'_>, value: &AstNode<'_>) -> Option<SymbolId> {
    let nodes = file.semantic.nodes();
    let AstKind::VariableDeclarator(declarator) = nodes.parent_kind(value.id()) else {
        return None;
    };
    let AstKind::VariableDeclaration(declaration) = nodes.parent_kind(nodes.parent_id(value.id()))
    else {
        return None;
    };
    let BindingPattern::BindingIdentifier(alias) = &declarator.id else {
        return None;
    };
    // The value's parent is the declarator, so the value is its `init`.
    if !declaration.kind.is_const() {
        return None;
    }
    alias.symbol_id.get()
}

/// The expression that copies the value: `{ ...acc }`, `[...acc, item]`, the global
/// `Object.assign({}, acc)`, `Array.from(acc)`, `structuredClone(acc)`, `new Map(acc)` and
/// `new Set(acc)`, and on an array `acc.concat(..)`, `.slice()`, `.toSorted()`, `.toReversed()`,
/// `.toSpliced(..)`, `.with(..)`.
fn copy_of(file: &File<'_>, value: &AstNode<'_>, is_array: bool) -> Option<Span> {
    let nodes = file.semantic.nodes();
    let parent = nodes.parent_node(value.id());
    let span = value.span();
    let is_first = |arguments: &[Argument<'_>]| {
        arguments
            .first()
            .is_some_and(|argument| argument.span() == span)
    };
    match parent.kind() {
        AstKind::SpreadElement(_) => match nodes.parent_kind(parent.id()) {
            AstKind::ObjectExpression(object) => Some(object.span),
            AstKind::ArrayExpression(array) => Some(array.span),
            _ => None,
        },
        AstKind::CallExpression(call) => {
            let callee = call.callee.get_inner_expression();
            let is_copy = match callee.get_member_expr() {
                Some(member) => {
                    let owner = member.object().get_inner_expression();
                    let target = call.arguments.first().and_then(Argument::as_expression);
                    match member.static_property_name()? {
                        // `Object.assign(acc, item)` fills the accumulator itself.
                        "assign" => {
                            is_global(file, owner, "Object")
                                && target.is_some_and(|target| {
                                    matches!(
                                        target.get_inner_expression(),
                                        Expression::ObjectExpression(_)
                                    )
                                })
                        }
                        "from" => is_global(file, owner, "Array") && is_first(&call.arguments),
                        _ => false,
                    }
                }
                None => is_global(file, callee, "structuredClone") && is_first(&call.arguments),
            };
            is_copy.then_some(call.span)
        }
        AstKind::NewExpression(new) => {
            let class = new.callee.get_inner_expression();
            let is_copy = (is_global(file, class, "Map") || is_global(file, class, "Set"))
                && is_first(&new.arguments);
            is_copy.then_some(new.span)
        }
        kind => {
            let member = kind.as_member_expression_kind()?;
            let is_copy = is_array
                && member.object().span() == span
                && matches!(
                    member.static_property_name()?.as_str(),
                    "concat" | "slice" | "toSorted" | "toReversed" | "toSpliced" | "with"
                );
            if !is_copy {
                return None;
            }
            let method = outer_assertion(file, parent);
            let AstKind::CallExpression(call) = nodes.parent_kind(method.id()) else {
                return None;
            };
            (call.callee.span() == method.span()).then_some(call.span)
        }
    }
}
