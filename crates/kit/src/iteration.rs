use oxc_ast::AstKind;
use oxc_ast::ast::{BinaryOperator, Expression, ForStatement};
use oxc_semantic::{AstNode, NodeId};
use oxc_span::{GetSpan, Span};

use crate::{File, one_line};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopKind {
    For,
    ForOf,
    ForAwaitOf,
    ForIn,
    While,
    DoWhile,
    /// A function given to `forEach`, `map` or another method of `PER_ITEM`.
    Callback,
    /// A function given to `.on(event, ..)`, `.addListener(..)` or `.addEventListener(..)`.
    Listener,
}

#[derive(Debug)]
pub struct LoopAround<'a> {
    /// The loop statement, or the call whose callback runs once per item or per event.
    pub node: &'a AstNode<'a>,
    pub kind: LoopKind,
    /// `orders` of `for (const o of orders)`, of `orders.forEach(..)` and of
    /// `for (let i = 0; i < orders.length; i++)`; `None` for `while` and listeners.
    pub over: Option<&'a Expression<'a>>,
    /// Loops, callbacks and listeners around the node, this one included.
    pub depth: u32,
}

/// Methods that call their first argument once per item. The receiver's type is not seen: the
/// `forEach` of a `Map` or a `Set` runs once per entry too.
const PER_ITEM: &[&str] = &[
    "forEach",
    "map",
    "flatMap",
    "filter",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "some",
    "every",
    "reduce",
    "reduceRight",
];

/// Methods that call their second argument once per event: `stream.on('data', ..)`.
const LISTENERS: &[&str] = &["on", "addListener", "addEventListener"];

/// The innermost loop around `node`. Stops at a function that is not a per-item callback or a
/// listener. Only the parts that run on every pass hold the node: the test, the update and the
/// body of `for`, not its `init`; the body of `for..of` and `for..in`, not the collection they
/// read once; the test and the body of `while` and `do`; the function of a callback, not the
/// receiver or the other arguments.
pub fn loop_around<'a>(file: &File<'a>, node: NodeId) -> Option<LoopAround<'a>> {
    let nodes = file.semantic.nodes();
    let mut child = nodes.get_node(node);
    let mut found = None;
    let mut depth = 0;
    for ancestor in nodes.ancestors(node) {
        let pass = match ancestor.kind() {
            AstKind::Function(_) | AstKind::ArrowFunctionExpression(_) => {
                let call = nodes.parent_node(ancestor.id());
                let Some((kind, over)) = callback(call.kind(), ancestor.span()) else {
                    break;
                };
                Some((call, kind, over))
            }
            AstKind::ForStatement(statement) => statement
                .init
                .as_ref()
                .is_none_or(|init| init.span() != child.span())
                .then(|| (ancestor, LoopKind::For, index_bound(statement))),
            AstKind::ForOfStatement(statement) if statement.right.span() != child.span() => {
                let kind = if statement.r#await {
                    LoopKind::ForAwaitOf
                } else {
                    LoopKind::ForOf
                };
                Some((ancestor, kind, Some(&statement.right)))
            }
            AstKind::ForInStatement(statement) if statement.right.span() != child.span() => {
                Some((ancestor, LoopKind::ForIn, Some(&statement.right)))
            }
            AstKind::WhileStatement(_) => Some((ancestor, LoopKind::While, None)),
            AstKind::DoWhileStatement(_) => Some((ancestor, LoopKind::DoWhile, None)),
            _ => None,
        };
        if let Some(pass) = pass {
            depth += 1;
            found.get_or_insert(pass);
        }
        child = ancestor;
    }
    found.map(|(node, kind, over)| LoopAround {
        node,
        kind,
        over,
        depth,
    })
}

impl LoopAround<'_> {
    /// The loop as a message names it: "`for..of` loop over `orders`", "`forEach` callback over
    /// `rows`", "`while` loop", "`on` listener".
    pub fn label(&self, file: &File<'_>) -> String {
        let method = || match self.node.kind() {
            AstKind::CallExpression(call) => call
                .callee
                .get_member_expr()
                .and_then(|member| member.static_property_name())
                .unwrap_or_default(),
            _ => "",
        };
        let name = match self.kind {
            LoopKind::For => "`for` loop".to_string(),
            LoopKind::ForOf => "`for..of` loop".to_string(),
            LoopKind::ForAwaitOf => "`for await` loop".to_string(),
            LoopKind::ForIn => "`for..in` loop".to_string(),
            LoopKind::While => "`while` loop".to_string(),
            LoopKind::DoWhile => "`do..while` loop".to_string(),
            LoopKind::Callback => format!("`{}` callback", method()),
            LoopKind::Listener => format!("`{}` listener", method()),
        };
        match self.over {
            Some(over) => format!(
                "{name} over `{}`",
                one_line(over.span().source_text(file.text))
            ),
            None => name,
        }
    }
}

/// The loop the function at `function` makes when `parent` is the call it is given to.
fn callback<'a>(
    parent: AstKind<'a>,
    function: Span,
) -> Option<(LoopKind, Option<&'a Expression<'a>>)> {
    let AstKind::CallExpression(call) = parent else {
        return None;
    };
    let member = call.callee.get_member_expr()?;
    let method = member.static_property_name()?;
    let is_at = |index: usize| {
        call.arguments
            .get(index)
            .is_some_and(|argument| argument.span() == function)
    };
    if PER_ITEM.contains(&method) && is_at(0) {
        Some((LoopKind::Callback, Some(member.object())))
    } else if LISTENERS.contains(&method) && is_at(1) {
        Some((LoopKind::Listener, None))
    } else {
        None
    }
}

/// `rows` of the test `i < rows.length`, `i <= rows.length` or `rows.length > i`.
fn index_bound<'a>(statement: &'a ForStatement<'a>) -> Option<&'a Expression<'a>> {
    let Expression::BinaryExpression(test) = statement.test.as_ref()?.get_inner_expression() else {
        return None;
    };
    let bound = match test.operator {
        BinaryOperator::LessThan | BinaryOperator::LessEqualThan => &test.right,
        BinaryOperator::GreaterThan | BinaryOperator::GreaterEqualThan => &test.left,
        _ => return None,
    };
    let length = bound.get_member_expr()?;
    (length.static_property_name()? == "length").then(|| length.object())
}
