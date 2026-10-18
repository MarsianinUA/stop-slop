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
        let skipped = match node.kind() {
            // `else if` stays on the level of its `if`, which reports the chain.
            AstKind::IfStatement(_) => is_else_if(file, node),
            AstKind::Function(_) | AstKind::ArrowFunctionExpression(_) => {
                block_body(node).is_none() || !is_callback(file, node)
            }
            _ => false,
        };
        // Its body is one level deeper than the blocks around it.
        if skipped || depth(file, node, self.max) != self.max {
            return;
        }
        // The head tells one block from another; a block without a head is told by its body.
        let span = match node.kind() {
            AstKind::IfStatement(statement) => {
                head(file, statement.span, statement.consequent.span())
            }
            AstKind::SwitchStatement(statement) => {
                let end = statement.discriminant.span().end;
                let close = file.text[end as usize..]
                    .find(')')
                    .map_or(0, |index| index as u32 + 1);
                Span::new(statement.span.start, end + close)
            }
            // `do` has its test after the body.
            AstKind::DoWhileStatement(_) => node.span(),
            kind => {
                loop_body(kind).map_or(node.span(), |body| head(file, node.span(), body.span()))
            }
        };
        cx.report(span)
            .arg("depth", self.max + 1)
            .arg("max", self.max);
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

/// How many blocks hold the node: the body of `if` and `else` (not `else if`), of a loop, of a
/// `switch`, of `try`, `catch` and `finally`, and of a callback, counted up to the function that
/// is not a callback, a field initializer or a static block. The count stops past `max`.
fn depth(file: &File<'_>, node: &AstNode<'_>, max: u32) -> u32 {
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
            AstKind::SwitchStatement(statement) => statement.discriminant.span() != child,
            AstKind::TryStatement(_) => true,
            AstKind::Function(_) | AstKind::ArrowFunctionExpression(_)
                if is_callback(file, ancestor) =>
            {
                block_body(ancestor) == Some(child)
            }
            AstKind::Function(_)
            | AstKind::ArrowFunctionExpression(_)
            | AstKind::StaticBlock(_)
            | AstKind::PropertyDefinition(_)
            | AstKind::AccessorProperty(_) => break,
            kind => loop_body(kind).is_some_and(|body| body.span() == child),
        };
        depth += u32::from(level);
        if depth > max {
            break;
        }
        child = ancestor.span();
    }
    depth
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

/// The block of a function; `(item) => item.id` holds none.
fn block_body(node: &AstNode<'_>) -> Option<Span> {
    match node.kind() {
        AstKind::Function(function) => function.body.as_ref().map(|body| body.span),
        AstKind::ArrowFunctionExpression(arrow) => match &arrow.body {
            ArrowFunctionBody::FunctionBody(body) => Some(body.span),
            _ => None,
        },
        _ => None,
    }
}
