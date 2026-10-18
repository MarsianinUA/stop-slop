use kit::oxc::ast::ast::{CallExpression, Expression, Statement};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// A word of the receiver's name that makes it a logger: `this.logger`, `console`.
    loggers: Vec<String>,
    /// A logger's method that writes a line.
    levels: Vec<String>,
}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::CatchClause, AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let (span, does) = match node.kind() {
            AstKind::CatchClause(clause) => (clause.span, self.swallows(&clause.body.body)),
            // `load(id).catch(() => {})` swallows the rejection the way `catch {}` does. From
            // `catch` to the end of the call: each link of a chain is its own place.
            AstKind::CallExpression(call) => match catch_callback(call) {
                Some((method, callback)) => (
                    Span::new(method.start, call.span.end),
                    self.callback_swallows(callback),
                ),
                None => return,
            },
            _ => return,
        };
        if let Some(does) = does {
            cx.report(span).arg("does", does);
        }
    }
}

impl Check {
    /// How a handler swallows the error, if it does: it `is empty` - nothing but `;` and
    /// comments (a comment explains the silence but does not handle the error), or it
    /// `only logs`.
    fn swallows(&self, statements: &[Statement<'_>]) -> Option<&'static str> {
        let mut statements = statements
            .iter()
            .filter(|statement| !matches!(statement, Statement::EmptyStatement(_)))
            .peekable();
        if statements.peek().is_none() {
            return Some("is empty");
        }
        statements
            .all(|statement| {
                matches!(statement, Statement::ExpressionStatement(statement)
                    if self.is_log(&statement.expression))
            })
            .then_some("only logs")
    }

    fn callback_swallows(&self, callback: &Expression<'_>) -> Option<&'static str> {
        match callback {
            Expression::ArrowFunctionExpression(arrow) => match arrow.get_function_body() {
                Some(body) => self.swallows(&body.statements),
                None => arrow
                    .get_expression()
                    .is_some_and(|value| self.is_log(value))
                    .then_some("only logs"),
            },
            Expression::FunctionExpression(function) => {
                self.swallows(&function.body.as_ref()?.statements)
            }
            _ => None,
        }
    }

    /// `this.logger.warn(..)`, `logger?.info(..)`, `await log.error(..)`.
    fn is_log(&self, expression: &Expression<'_>) -> bool {
        let mut expression = expression.get_inner_expression();
        if let Expression::AwaitExpression(awaited) = expression {
            expression = &awaited.argument;
        }
        call_of(expression).is_some_and(|call| is_log_call(call, &self.loggers, &self.levels))
    }
}

/// The method name and the callback of `promise.catch(cb)` (also `?.catch`). Only an arrow or a
/// function written right there is judged; a function passed by name (`.catch(log)`) is not seen.
fn catch_callback<'a>(call: &'a CallExpression<'a>) -> Option<(Span, &'a Expression<'a>)> {
    let Expression::StaticMemberExpression(method) = call.callee.get_inner_expression() else {
        return None;
    };
    let [callback] = call.arguments.as_slice() else {
        return None;
    };
    if method.property.name != "catch" {
        return None;
    }
    Some((
        method.property.span,
        callback.as_expression()?.get_inner_expression(),
    ))
}
