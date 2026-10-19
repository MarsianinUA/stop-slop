use std::collections::HashMap;

use kit::oxc::ast::ast::{Expression, IfStatement, LogicalExpression, LogicalOperator, Statement};
use kit::oxc::semantic::NodeId;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    max: u32,
}

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let nodes = file.semantic.nodes();
        let mut scores: HashMap<NodeId, u32> = HashMap::new();
        for node in nodes.iter() {
            let Some(cost) = cost(file, node) else {
                continue;
            };
            let Some((function, nesting)) = owner(file, node) else {
                continue;
            };
            let points = cost.flat + if cost.nests { 1 + nesting } else { 0 };
            *scores.entry(function).or_default() += points;
        }
        for (function, complexity) in scores {
            if complexity > self.max {
                let (name, span) = named(file, nodes.get_node(function));
                cx.report(span)
                    .arg("function", name)
                    .arg("complexity", complexity)
                    .arg("max", self.max);
            }
        }
    }
}

/// The points of one node, as SonarJS counts cognitive complexity (rule S3776).
struct Cost {
    /// Points the node always adds: `else`, `else if`, a labeled `break` or `continue`, each
    /// run of `&&` in a chain of `&&`, `||` and `??` (those two are free).
    flat: u32,
    /// One point plus one for each level of nesting: `if`, `?:`, `switch`, loops, `catch`.
    nests: bool,
}

fn cost(file: &File<'_>, node: &AstNode<'_>) -> Option<Cost> {
    let cost = |flat, nests| Some(Cost { flat, nests });
    match node.kind() {
        AstKind::IfStatement(statement) => {
            let else_if = is_else_if(file, node);
            cost(
                u32::from(else_if) + u32::from(else_body(statement).is_some()),
                !else_if,
            )
        }
        AstKind::ConditionalExpression(_)
        | AstKind::SwitchStatement(_)
        | AstKind::CatchClause(_) => cost(0, true),
        AstKind::BreakStatement(statement) if statement.label.is_some() => cost(1, false),
        AstKind::ContinueStatement(statement) if statement.label.is_some() => cost(1, false),
        // The whole chain is counted once, at its outermost operator.
        AstKind::LogicalExpression(logical)
            if !matches!(
                file.semantic.nodes().parent_kind(node.id()),
                AstKind::LogicalExpression(_)
            ) =>
        {
            let runs = and_runs(logical, &mut None);
            cost(runs, false).filter(|_| runs > 0)
        }
        kind if loop_body(kind).is_some() => cost(0, true),
        _ => None,
    }
}

fn else_body<'b, 'a>(statement: &'b IfStatement<'a>) -> Option<&'b Statement<'a>> {
    statement
        .alternate
        .as_ref()
        .filter(|alternate| !matches!(alternate, Statement::IfStatement(_)))
}

fn loop_body<'a>(kind: AstKind<'a>) -> Option<&'a Statement<'a>> {
    match kind {
        AstKind::ForStatement(statement) => Some(&statement.body),
        AstKind::ForInStatement(statement) => Some(&statement.body),
        AstKind::ForOfStatement(statement) => Some(&statement.body),
        AstKind::WhileStatement(statement) => Some(&statement.body),
        AstKind::DoWhileStatement(statement) => Some(&statement.body),
        _ => None,
    }
}

fn is_else_if(file: &File<'_>, node: &AstNode<'_>) -> bool {
    matches!(file.semantic.nodes().parent_kind(node.id()), AstKind::IfStatement(parent)
        if parent.alternate.as_ref().is_some_and(|alternate| alternate.span() == node.span()))
}

/// The `&&` operators of the chain, left to right, that do not follow another `&&`:
/// `a && b && c` has one run, `a && b || c && d` two, `a || b ?? c` none.
fn and_runs(logical: &LogicalExpression<'_>, previous: &mut Option<LogicalOperator>) -> u32 {
    let side = |side: &Expression<'_>, previous: &mut Option<LogicalOperator>| match side {
        Expression::LogicalExpression(inner) => and_runs(inner, previous),
        _ => 0,
    };
    let left = side(&logical.left, previous);
    let starts =
        logical.operator == LogicalOperator::And && *previous != Some(LogicalOperator::And);
    *previous = Some(logical.operator);
    left + u32::from(starts) + side(&logical.right, previous)
}

/// The innermost function around the node, and how many levels nest the node inside it: the
/// branches of `if` (an `else if` stays on the level of its `if`) and of `?:`, the bodies of
/// loops and `catch`, the cases of `switch`. A nested function counts on its own, from level 0;
/// a class field or static block outside any function is not scored.
fn owner(file: &File<'_>, node: &AstNode<'_>) -> Option<(NodeId, u32)> {
    let mut nesting = 0;
    let mut child = node.span();
    for ancestor in file.semantic.nodes().ancestors(node.id()) {
        let nests = match ancestor.kind() {
            AstKind::Function(_) | AstKind::ArrowFunctionExpression(_) => {
                return Some((ancestor.id(), nesting));
            }
            AstKind::PropertyDefinition(_)
            | AstKind::AccessorProperty(_)
            | AstKind::StaticBlock(_) => return None,
            AstKind::IfStatement(statement) => {
                statement.consequent.span() == child
                    || else_body(statement).is_some_and(|body| body.span() == child)
            }
            AstKind::ConditionalExpression(conditional) => conditional.test.span() != child,
            AstKind::SwitchCase(_) => true,
            AstKind::CatchClause(clause) => clause.body.span == child,
            kind => loop_body(kind).is_some_and(|body| body.span() == child),
        };
        nesting += u32::from(nests);
        child = ancestor.span();
    }
    None
}

/// How the message names the function, and its head, where the finding sits: from its name, or
/// the key or variable that holds it, to its body (`build(rows: Row[]): number`).
fn named(file: &File<'_>, function: &AstNode<'_>) -> (String, Span) {
    let (name, body) = match function.kind() {
        AstKind::Function(declared) => (
            declared.id.as_ref().map(|id| id.span),
            declared.body.as_ref().map(|body| body.span.start),
        ),
        AstKind::ArrowFunctionExpression(arrow) => (None, Some(arrow.body.span().start)),
        _ => (None, None),
    };
    let holder = outer_assertion(file, function).id();
    let name = name.or_else(|| match file.semantic.nodes().parent_kind(holder) {
        AstKind::MethodDefinition(method) => Some(method.key.span()),
        AstKind::PropertyDefinition(field) => Some(field.key.span()),
        AstKind::ObjectProperty(property) => Some(property.key.span()),
        AstKind::VariableDeclarator(declarator) => Some(declarator.id.span()),
        _ => None,
    });
    let start = name.map_or(function.span().start, |name| name.start);
    let head = Span::new(start, body.unwrap_or(function.span().end)).source_text(file.text);
    let span = Span::sized(start, head.trim_end().len() as u32);
    let name = match name {
        Some(name) => format!("`{}`", one_line(name.source_text(file.text))),
        None if is_callback(file, function) => "This callback".to_string(),
        None => "This function".to_string(),
    };
    (name, span)
}
