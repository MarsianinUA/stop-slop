use kit::oxc::ast::ast::{ArrowFunctionBody, Statement};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    max: u32,
}

impl Rule for Check {
    const NODES: &'static [AstType] = &[
        AstType::IfStatement,
        AstType::ForStatement,
        AstType::ForInStatement,
        AstType::ForOfStatement,
        AstType::WhileStatement,
        AstType::DoWhileStatement,
        AstType::SwitchStatement,
        AstType::TryStatement,
        AstType::Function,
        AstType::ArrowFunctionExpression,
    ];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        // The head tells one block from another; a block without a head is told by its body.
        let span = match node.kind() {
            // `else if` stays on the level of its `if`, which reports the chain.
            AstKind::IfStatement(_) if is_else_if(file, node) => return,
            AstKind::IfStatement(statement) => {
                head(file, statement.span, statement.consequent.span())
            }
            AstKind::ForStatement(statement) => head(file, statement.span, statement.body.span()),
            AstKind::ForInStatement(statement) => head(file, statement.span, statement.body.span()),
            AstKind::ForOfStatement(statement) => head(file, statement.span, statement.body.span()),
            AstKind::WhileStatement(statement) => head(file, statement.span, statement.body.span()),
            AstKind::SwitchStatement(statement) => {
                let rest = &file.text[statement.discriminant.span().end as usize..];
                let close = rest.find(')').map_or(0, |index| index as u32 + 1);
                Span::new(
                    statement.span.start,
                    statement.discriminant.span().end + close,
                )
            }
            AstKind::Function(_) | AstKind::ArrowFunctionExpression(_)
                if !is_block_callback(file, node) =>
            {
                return;
            }
            _ => node.span(),
        };
        // Its body is one level deeper than the blocks around it.
        if depth(file, node) == self.max {
            cx.report(span)
                .arg("depth", self.max + 1)
                .arg("max", self.max);
        }
    }
}

fn is_else_if(file: &File<'_>, node: &AstNode<'_>) -> bool {
    matches!(file.semantic.nodes().parent_kind(node.id()), AstKind::IfStatement(parent)
        if parent.alternate.as_ref().is_some_and(|alternate| alternate.span() == node.span()))
}

/// `if (order.isPaid)` of `if (order.isPaid) {`: from the keyword to the body.
fn head(file: &File<'_>, statement: Span, body: Span) -> Span {
    let text = Span::new(statement.start, body.start).source_text(file.text);
    Span::sized(statement.start, text.trim_end().len() as u32)
}

/// A callback with a block body; `(item) => item.id` holds no block.
fn is_block_callback(file: &File<'_>, node: &AstNode<'_>) -> bool {
    match node.kind() {
        AstKind::Function(_) => is_callback(file, node),
        AstKind::ArrowFunctionExpression(arrow) => {
            matches!(arrow.body, ArrowFunctionBody::FunctionBody(_)) && is_callback(file, node)
        }
        _ => false,
    }
}

/// How many blocks hold the node: the body of `if` and `else` (not `else if`), of a loop, of a
/// `switch`, of `try`, `catch` and `finally`, and of a callback, counted up to the function that
/// is not a callback, a field initializer or a static block.
fn depth(file: &File<'_>, node: &AstNode<'_>) -> u32 {
    let mut depth = 0;
    let mut child = node.span();
    for ancestor in file.semantic.nodes().ancestors(node.id()) {
        let level = match ancestor.kind() {
            AstKind::IfStatement(statement) => {
                statement.consequent.span() == child
                    || statement.alternate.as_ref().is_some_and(|alternate| {
                        alternate.span() == child && !matches!(alternate, Statement::IfStatement(_))
                    })
            }
            AstKind::ForStatement(statement) => statement.body.span() == child,
            AstKind::ForInStatement(statement) => statement.body.span() == child,
            AstKind::ForOfStatement(statement) => statement.body.span() == child,
            AstKind::WhileStatement(statement) => statement.body.span() == child,
            AstKind::DoWhileStatement(statement) => statement.body.span() == child,
            AstKind::SwitchStatement(statement) => statement.discriminant.span() != child,
            AstKind::TryStatement(_) => true,
            AstKind::Function(function) if is_callback(file, ancestor) => function
                .body
                .as_ref()
                .is_some_and(|body| body.span == child),
            AstKind::ArrowFunctionExpression(arrow) if is_callback(file, ancestor) => {
                matches!(arrow.body, ArrowFunctionBody::FunctionBody(_))
                    && arrow.body.span() == child
            }
            AstKind::Function(_)
            | AstKind::ArrowFunctionExpression(_)
            | AstKind::StaticBlock(_)
            | AstKind::PropertyDefinition(_)
            | AstKind::AccessorProperty(_) => break,
            _ => false,
        };
        depth += u32::from(level);
        child = ancestor.span();
    }
    depth
}
