use kit::oxc::ast::ast::{
    AssignmentOperator, AssignmentTarget, BindingPattern, CallExpression, Class, ClassElement,
    Expression, FormalParameters, MethodDefinitionKind, ObjectPropertyKind, Statement,
};
use kit::oxc::semantic::{NodeId, SymbolId};
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
                .arg("function", call.callee.span().source_text(file.text));
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
                .is_some_and(|body| all_return(&body.statements, &mut results)),
        },
        AstKind::Function(function) => function
            .body
            .as_ref()
            .is_some_and(|body| all_return(&body.statements, &mut results)),
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

/// The function of this file that the call runs: a function declared with a body, a `const`
/// holding a function or an arrow, or `this.m(..)` - a method with a body or a function field of
/// the class whose `this` the call sees. A binding that is written to may hold another function.
fn local_callee<'a>(file: &File<'a>, call: &CallExpression<'a>) -> Option<&'a AstNode<'a>> {
    let nodes = file.semantic.nodes();
    let scoping = file.semantic.scoping();
    match call.callee.get_inner_expression() {
        Expression::Identifier(id) => {
            let symbol = scoping.get_reference(id.reference_id()).symbol_id()?;
            if scoping
                .get_resolved_references(symbol)
                .any(|reference| reference.is_write())
            {
                return None;
            }
            let declaration = scoping.symbol_declaration(symbol);
            match nodes.kind(declaration) {
                AstKind::Function(function) if function.body.is_some() => {
                    Some(nodes.get_node(declaration))
                }
                AstKind::VariableDeclarator(declarator) => {
                    function_value(file, declarator.init.as_ref()?)
                }
                _ => None,
            }
        }
        Expression::StaticMemberExpression(member)
            if matches!(member.object, Expression::ThisExpression(_)) =>
        {
            let name = member.property.name.as_str();
            this_class(file, call.node_id.get())?
                .body
                .body
                .iter()
                .find_map(|element| match element {
                    ClassElement::MethodDefinition(method)
                        if !method.r#static
                            && method.kind == MethodDefinitionKind::Method
                            && method.value.body.is_some()
                            && method.key.static_name().is_some_and(|key| key == name) =>
                    {
                        Some(nodes.get_node(method.value.node_id.get()))
                    }
                    ClassElement::PropertyDefinition(field)
                        if !field.r#static
                            && field.key.static_name().is_some_and(|key| key == name) =>
                    {
                        function_value(file, field.value.as_ref()?)
                    }
                    _ => None,
                })
        }
        _ => None,
    }
}

fn function_value<'a>(file: &File<'a>, value: &Expression<'a>) -> Option<&'a AstNode<'a>> {
    let id = match value.get_inner_expression() {
        Expression::ArrowFunctionExpression(arrow) => arrow.node_id.get(),
        Expression::FunctionExpression(function) => function.node_id.get(),
        _ => return None,
    };
    Some(file.semantic.nodes().get_node(id))
}

/// The class whose instance is `this` at the node: up through arrows to a non-static method or
/// field. Any other function in between has its own `this`, also one inside a field's arrow.
fn this_class<'a>(file: &File<'a>, node: NodeId) -> Option<&'a Class<'a>> {
    let nodes = file.semantic.nodes();
    for ancestor in nodes.ancestors(node) {
        match ancestor.kind() {
            AstKind::Function(_) => {
                let is_member = match nodes.parent_kind(ancestor.id()) {
                    AstKind::MethodDefinition(method) => !method.r#static,
                    AstKind::PropertyDefinition(field) => !field.r#static,
                    _ => false,
                };
                if !is_member {
                    return None;
                }
            }
            AstKind::MethodDefinition(method) if method.r#static => return None,
            AstKind::PropertyDefinition(field) if field.r#static => return None,
            AstKind::StaticBlock(_) => return None,
            AstKind::Class(class) => return Some(class),
            _ => {}
        }
    }
    None
}
