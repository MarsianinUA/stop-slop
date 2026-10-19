use kit::oxc::ast::ast::{Argument, Expression, FunctionBody, PropertyKey, Statement};
use kit::oxc::semantic::SymbolId;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

/// How code inside a function names that function.
enum Own<'a> {
    /// `tick` of `function tick() {}` or `const tick = () => {}`.
    Binding(SymbolId),
    /// `this.tick` of a method or a field `tick = () => {}`.
    Member(&'a str),
}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::CallExpression(call) = node.kind() else {
            return;
        };
        let file = cx.file();
        if global_member(file, &call.callee) != Some(("process", "nextTick")) {
            return;
        }
        let Some(callback) = call.arguments.first().and_then(Argument::as_expression) else {
            return;
        };
        // Only the innermost function: from a callback inside it, the next pass waits for
        // whatever calls that callback, such as I/O.
        let Some(function) = file
            .semantic
            .nodes()
            .ancestors(node.id())
            .find(|ancestor| ancestor.kind().is_function_like())
        else {
            return;
        };
        let Some((own, name)) = own_name(file, function) else {
            return;
        };
        if passes(file, callback, &own) {
            cx.report(call.span).arg("name", name);
        }
    }
}

fn own_name<'a>(file: &File<'a>, function: &AstNode<'a>) -> Option<(Own<'a>, &'a str)> {
    if let AstKind::Function(function) = function.kind()
        && let Some(id) = &function.id
    {
        return Some((Own::Binding(id.symbol_id()), id.name.as_str()));
    }
    let key = match file.semantic.nodes().parent_kind(function.id()) {
        AstKind::VariableDeclarator(declarator) => {
            let id = declarator.id.get_binding_identifier()?;
            return Some((Own::Binding(id.symbol_id()), id.name.as_str()));
        }
        AstKind::MethodDefinition(method) => &method.key,
        AstKind::PropertyDefinition(field) => &field.key,
        _ => return None,
    };
    let PropertyKey::StaticIdentifier(key) = key else {
        return None;
    };
    Some((Own::Member(key.name.as_str()), key.name.as_str()))
}

/// The function itself (`tick`, `this.tick`, `tick.bind(this)`), or a function that only calls
/// it: `() => tick()`, `() => this.tick()`, `function () { tick(); }`.
fn passes(file: &File<'_>, callback: &Expression<'_>, own: &Own<'_>) -> bool {
    let called = match callback.get_inner_expression() {
        Expression::ArrowFunctionExpression(arrow) => arrow
            .get_expression()
            .or_else(|| arrow.get_function_body().and_then(only_expression)),
        // A `function` has its own `this`: `this.tick()` in it does not call the method.
        Expression::FunctionExpression(_) if matches!(own, Own::Member(_)) => return false,
        Expression::FunctionExpression(function) => {
            function.body.as_deref().and_then(only_expression)
        }
        callback => return refers_to(file, callback, own),
    };
    called
        .and_then(call_of)
        .is_some_and(|call| refers_to(file, &call.callee, own))
}

fn only_expression<'b, 'a>(body: &'b FunctionBody<'a>) -> Option<&'b Expression<'a>> {
    match body.statements.as_slice() {
        [Statement::ExpressionStatement(statement)] => Some(&statement.expression),
        [Statement::ReturnStatement(statement)] => statement.argument.as_ref(),
        _ => None,
    }
}

fn refers_to(file: &File<'_>, expression: &Expression<'_>, own: &Own<'_>) -> bool {
    match (expression.get_inner_expression(), own) {
        (Expression::Identifier(id), Own::Binding(symbol)) => {
            file.semantic
                .scoping()
                .get_reference(id.reference_id())
                .symbol_id()
                == Some(*symbol)
        }
        (Expression::StaticMemberExpression(member), Own::Member(name)) => {
            matches!(member.object, Expression::ThisExpression(_)) && member.property.name == *name
        }
        (expression, _) => call_of(expression).is_some_and(|call| {
            call.callee.get_member_expr().is_some_and(|bind| {
                bind.static_property_name() == Some("bind") && refers_to(file, bind.object(), own)
            })
        }),
    }
}
