use kit::oxc::ast::ast::{Argument, Expression, FunctionBody, Statement};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let AstKind::CallExpression(call) = node.kind() else {
            return;
        };
        let Expression::StaticMemberExpression(callee) = call.callee.get_inner_expression() else {
            return;
        };
        if callee.property.name != "all" || !is_global(file, &callee.object, "Promise") {
            return;
        }
        let Some(map) = call
            .arguments
            .first()
            .and_then(Argument::as_expression)
            .and_then(call_of)
        else {
            return;
        };
        let Expression::StaticMemberExpression(method) = map.callee.get_inner_expression() else {
            return;
        };
        if method.property.name != "map"
            || map
                .arguments
                .first()
                .and_then(Argument::as_expression)
                .is_some_and(settles_itself)
        {
            return;
        }
        // `Promise.all(orders.map`: the list names the place; the callback may change.
        cx.report(Span::new(call.span.start, method.span.end)).arg(
            "items",
            one_line(method.object.span().source_text(file.text)),
        );
    }
}

/// Each item handles its own failure, so `Promise.all` never rejects:
/// `(id) => save(id).catch(..)`, a body that is only that call (a statement, `await` or `return`),
/// or a body of one `try` with a `catch`.
fn settles_itself(callback: &Expression<'_>) -> bool {
    match callback.get_inner_expression() {
        Expression::ArrowFunctionExpression(arrow) => match arrow.get_function_body() {
            Some(body) => body_settles(body),
            None => arrow.get_expression().is_some_and(is_catch_call),
        },
        Expression::FunctionExpression(function) => {
            function.body.as_deref().is_some_and(body_settles)
        }
        _ => false,
    }
}

fn body_settles(body: &FunctionBody<'_>) -> bool {
    match body.statements.as_slice() {
        [Statement::ExpressionStatement(statement)] => is_catch_call(&statement.expression),
        [Statement::ReturnStatement(statement)] => {
            statement.argument.as_ref().is_some_and(is_catch_call)
        }
        [Statement::TryStatement(statement)] => statement.handler.is_some(),
        _ => false,
    }
}

/// `save(id).catch(..)`, also under `await`, through `?.` and with `.finally(..)` after it.
fn is_catch_call(expression: &Expression<'_>) -> bool {
    let mut expression = expression.get_inner_expression();
    if let Expression::AwaitExpression(awaited) = expression {
        expression = &awaited.argument;
    }
    let mut call = call_of(expression);
    while let Some(link) = call {
        let Expression::StaticMemberExpression(method) = link.callee.get_inner_expression() else {
            return false;
        };
        match method.property.name.as_str() {
            "catch" => return true,
            "finally" => call = call_of(&method.object),
            _ => return false,
        }
    }
    false
}
