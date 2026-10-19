use kit::oxc::ast::ast::{
    Argument, ArrayExpressionElement, AssignmentOperator, AssignmentTarget, Expression,
    ObjectPropertyKind,
};
use kit::oxc::semantic::SymbolId;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::AssignmentExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::AssignmentExpression(assignment) = node.kind() else {
            return;
        };
        let AssignmentTarget::AssignmentTargetIdentifier(target) = &assignment.left else {
            return;
        };
        if assignment.operator != AssignmentOperator::Assign {
            return;
        }
        let file = cx.file();
        let scoping = file.semantic.scoping();
        let Some(symbol) = scoping.get_reference(target.reference_id()).symbol_id() else {
            return;
        };
        let name = target.name.as_str();
        let Some(copy) = copy(file, &assignment.right, symbol, name) else {
            return;
        };
        let Some(around) = loop_around(file, node.id()) else {
            return;
        };
        // A variable of `for (let all = []; ..)` keeps its value from pass to pass; one declared
        // in the body, or by `for (let row of rows)`, starts anew on each pass.
        let pass = match around.node.kind() {
            AstKind::ForStatement(statement) => statement.body.span(),
            _ => around.node.span(),
        };
        if pass.contains_inclusive(scoping.symbol_span(symbol)) {
            return;
        }
        cx.report(assignment.span)
            .arg("name", name)
            .arg("copy", copy);
    }
}

/// The copy of the variable that `value` makes, one that grows: `[...all, item]`,
/// `all.concat(page)`, `{ ...byId, [order.id]: order }`, `Buffer.concat([buffer, chunk])`. An
/// object spread with fixed keys (`{ ...state, count }`) stays the same size from pass to pass.
fn copy(file: &File<'_>, value: &Expression<'_>, symbol: SymbolId, name: &str) -> Option<String> {
    let scoping = file.semantic.scoping();
    let is_self = |expression: &Expression<'_>| {
        matches!(expression.get_inner_expression(), Expression::Identifier(id)
            if scoping.get_reference(id.reference_id()).symbol_id() == Some(symbol))
    };
    match value.get_inner_expression() {
        Expression::ArrayExpression(array) => {
            let spreads = array.elements.iter().any(|element| {
                matches!(element, ArrayExpressionElement::SpreadElement(spread)
                    if is_self(&spread.argument))
            });
            (spreads && array.elements.len() > 1).then(|| format!("[...{name}, ..]"))
        }
        Expression::ObjectExpression(object) => {
            let spreads = object.properties.iter().any(|property| {
                matches!(property, ObjectPropertyKind::SpreadProperty(spread)
                    if is_self(&spread.argument))
            });
            let grows = object.properties.iter().any(|property| {
                matches!(property, ObjectPropertyKind::ObjectProperty(property) if property.computed)
            });
            (spreads && grows).then(|| format!("{{ ...{name}, [key]: .. }}"))
        }
        value => {
            let call = call_of(value)?;
            if global_member(file, &call.callee) == Some(("Buffer", "concat")) {
                let Some(Expression::ArrayExpression(parts)) = call
                    .arguments
                    .first()
                    .and_then(Argument::as_expression)
                    .map(Expression::get_inner_expression)
                else {
                    return None;
                };
                let has_self = parts
                    .elements
                    .iter()
                    .any(|part| part.as_expression().is_some_and(is_self));
                return (has_self && parts.elements.len() > 1)
                    .then(|| format!("Buffer.concat([{name}, ..])"));
            }
            let member = call.callee.get_member_expr()?;
            let is_concat = member.static_property_name() == Some("concat")
                && is_self(member.object())
                && !call.arguments.is_empty()
                && starts_as_array(file, member.object(), symbol);
            is_concat.then(|| format!("{name}.concat(..)"))
        }
    }
}

/// `concat` copies an array but joins a string: the type or the first value of the variable
/// tells which (`let all: Order[] = ..`, `let all = []`).
fn starts_as_array(file: &File<'_>, variable: &Expression<'_>, symbol: SymbolId) -> bool {
    let declaration = file.semantic.scoping().symbol_declaration(symbol);
    is_known_array(file, variable)
        || matches!(file.semantic.nodes().kind(declaration), AstKind::VariableDeclarator(declarator)
            if declarator.init.as_ref().is_some_and(|init| is_known_array(file, init)))
}
