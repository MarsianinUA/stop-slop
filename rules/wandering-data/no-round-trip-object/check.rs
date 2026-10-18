use kit::oxc::ast::ast::{
    AssignmentOperator, AssignmentTarget, BindingPattern, Expression, FormalParameters,
    ObjectPropertyKind, Statement,
};
use kit::oxc::semantic::SymbolId;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::AssignmentExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let AstKind::AssignmentExpression(assignment) = node.kind() else {
            return;
        };
        let AssignmentTarget::AssignmentTargetIdentifier(target) = &assignment.left else {
            return;
        };
        if assignment.operator != AssignmentOperator::Assign {
            return;
        }
        let Expression::CallExpression(call) = without_await(&assignment.right) else {
            return;
        };
        let scoping = file.semantic.scoping();
        let Some(symbol) = scoping.get_reference(target.reference_id()).symbol_id() else {
            return;
        };
        let Some(function) = local_callee(file, call) else {
            return;
        };
        let Some(params) = params_of(function) else {
            return;
        };
        // A spread argument shifts the parameters after it.
        let round_trips = call
            .arguments
            .iter()
            .take_while(|argument| !argument.is_spread())
            .zip(&params.items)
            .any(|(argument, parameter)| {
                argument
                    .as_expression()
                    .is_some_and(|argument| is_symbol(file, argument, symbol))
                    && matches!(&parameter.pattern, BindingPattern::BindingIdentifier(binding)
                        if binding.symbol_id.get().is_some_and(|param| returns_param(file, function, param)))
            });
        if round_trips {
            cx.report(assignment.span)
                .arg("name", target.name.as_str())
                .arg(
                    "function",
                    one_line(call.callee.span().source_text(file.text)),
                );
        }
    }
}

/// Every result of the function is the parameter, `{ ...param, .. }` or a `?:` of them, and it
/// has one at least. A function that hands back the parameter itself and never spreads it counts
/// only when it writes to the parameter: otherwise it checks or narrows the value
/// (`row = requireRow(row)`).
fn returns_param(file: &File<'_>, function: &AstNode<'_>, param: SymbolId) -> bool {
    let mut results = Vec::new();
    let complete = match function.kind() {
        AstKind::ArrowFunctionExpression(arrow) => match arrow.get_expression() {
            Some(expression) => {
                results.push(expression);
                true
            }
            None => arrow
                .get_function_body()
                .is_some_and(|body| body_returns(&body.statements, &mut results)),
        },
        AstKind::Function(function) => function
            .body
            .as_ref()
            .is_some_and(|body| body_returns(&body.statements, &mut results)),
        _ => false,
    };
    if !complete || results.is_empty() {
        return false;
    }
    let Some(spread) = results.into_iter().try_fold(false, |spread, result| {
        Some(spread | shape(file, result, param)?)
    }) else {
        return false;
    };
    spread
        || file
            .semantic
            .scoping()
            .get_resolved_references(param)
            .any(|reference| member_write(file, reference.node_id()).is_some())
}

/// Collects the values of the `return` statements; false on a bare `return;` or when the body may
/// run off its end, whose result is `undefined` too.
fn body_returns<'a>(
    statements: &'a [Statement<'a>],
    results: &mut Vec<&'a Expression<'a>>,
) -> bool {
    all_return(statements, results) && ends(statements)
}

/// Collects the values of the `return` statements, outside nested functions; false on a bare
/// `return;`. A walk of the body, not of the whole file.
fn all_return<'a>(statements: &'a [Statement<'a>], results: &mut Vec<&'a Expression<'a>>) -> bool {
    statements
        .iter()
        .all(|statement| statement_returns(statement, results))
}

fn statement_returns<'a>(
    statement: &'a Statement<'a>,
    results: &mut Vec<&'a Expression<'a>>,
) -> bool {
    match statement {
        Statement::ReturnStatement(statement) => match &statement.argument {
            Some(argument) => {
                results.push(argument);
                true
            }
            None => false,
        },
        Statement::BlockStatement(block) => all_return(&block.body, results),
        Statement::IfStatement(statement) => {
            statement_returns(&statement.consequent, results)
                && statement
                    .alternate
                    .as_ref()
                    .is_none_or(|alternate| statement_returns(alternate, results))
        }
        Statement::TryStatement(statement) => {
            all_return(&statement.block.body, results)
                && statement
                    .handler
                    .as_ref()
                    .is_none_or(|handler| all_return(&handler.body.body, results))
                && statement
                    .finalizer
                    .as_ref()
                    .is_none_or(|finalizer| all_return(&finalizer.body, results))
        }
        Statement::SwitchStatement(statement) => statement
            .cases
            .iter()
            .all(|case| all_return(&case.consequent, results)),
        Statement::ForStatement(statement) => statement_returns(&statement.body, results),
        Statement::ForInStatement(statement) => statement_returns(&statement.body, results),
        Statement::ForOfStatement(statement) => statement_returns(&statement.body, results),
        Statement::WhileStatement(statement) => statement_returns(&statement.body, results),
        Statement::DoWhileStatement(statement) => statement_returns(&statement.body, results),
        Statement::LabeledStatement(statement) => statement_returns(&statement.body, results),
        _ => true,
    }
}

/// The statements never run off their end: the last one returns or throws, or is a block, an `if`
/// with `else`, a `try` or a `switch` with `default` and no `break` that does so on every way out.
/// A loop may run no times.
fn ends(statements: &[Statement<'_>]) -> bool {
    statements.last().is_some_and(ends_with)
}

fn ends_with(statement: &Statement<'_>) -> bool {
    match statement {
        Statement::ReturnStatement(_) | Statement::ThrowStatement(_) => true,
        Statement::BlockStatement(block) => ends(&block.body),
        Statement::IfStatement(statement) => {
            ends_with(&statement.consequent) && statement.alternate.as_ref().is_some_and(ends_with)
        }
        Statement::TryStatement(statement) => {
            statement
                .finalizer
                .as_ref()
                .is_some_and(|finalizer| ends(&finalizer.body))
                || ends(&statement.block.body)
                    && statement
                        .handler
                        .as_ref()
                        .is_none_or(|handler| ends(&handler.body.body))
        }
        // A case without its own end falls through to the next one.
        Statement::SwitchStatement(statement) => {
            statement.cases.iter().any(|case| case.is_default_case())
                && statement
                    .cases
                    .last()
                    .is_some_and(|case| ends(&case.consequent))
                && !statement
                    .cases
                    .iter()
                    .any(|case| case.consequent.iter().any(breaks))
        }
        _ => false,
    }
}

/// A `break` that leaves the `switch` around it: not one inside a nested loop or `switch`.
fn breaks(statement: &Statement<'_>) -> bool {
    match statement {
        Statement::BreakStatement(_) => true,
        Statement::BlockStatement(block) => block.body.iter().any(breaks),
        Statement::IfStatement(statement) => {
            breaks(&statement.consequent) || statement.alternate.as_ref().is_some_and(breaks)
        }
        Statement::TryStatement(statement) => {
            statement.block.body.iter().any(breaks)
                || statement
                    .handler
                    .as_ref()
                    .is_some_and(|handler| handler.body.body.iter().any(breaks))
                || statement
                    .finalizer
                    .as_ref()
                    .is_some_and(|finalizer| finalizer.body.iter().any(breaks))
        }
        Statement::LabeledStatement(statement) => breaks(&statement.body),
        _ => false,
    }
}

fn is_symbol(file: &File<'_>, expression: &Expression<'_>, symbol: SymbolId) -> bool {
    matches!(expression.get_inner_expression(), Expression::Identifier(id)
        if file.semantic.scoping().get_reference(id.reference_id()).symbol_id() == Some(symbol))
}

/// `Some(false)` for the parameter itself, `Some(true)` for `{ ...param, .. }`; a `?:` of them is
/// a spread when a branch is.
fn shape(file: &File<'_>, result: &Expression<'_>, param: SymbolId) -> Option<bool> {
    match without_await(result) {
        result if is_symbol(file, result, param) => Some(false),
        Expression::ConditionalExpression(conditional) => Some(
            shape(file, &conditional.consequent, param)?
                | shape(file, &conditional.alternate, param)?,
        ),
        Expression::ObjectExpression(object) => object
            .properties
            .iter()
            .any(|property| {
                matches!(property, ObjectPropertyKind::SpreadProperty(spread) if is_symbol(file, &spread.argument, param))
            })
            .then_some(true),
        _ => None,
    }
}

fn without_await<'a>(expression: &'a Expression<'a>) -> &'a Expression<'a> {
    match expression.get_inner_expression() {
        Expression::AwaitExpression(expression) => expression.argument.get_inner_expression(),
        expression => expression,
    }
}

fn params_of<'a>(function: &AstNode<'a>) -> Option<&'a FormalParameters<'a>> {
    match function.kind() {
        AstKind::Function(function) => Some(&function.params),
        AstKind::ArrowFunctionExpression(arrow) => Some(&arrow.params),
        _ => None,
    }
}
