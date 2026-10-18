use kit::oxc::ast::ast::{Expression, LogicalOperator, UnaryOperator};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    max: u32,
}

impl Rule for Check {
    const NODES: &'static [AstType] = &[
        AstType::IfStatement,
        AstType::WhileStatement,
        AstType::DoWhileStatement,
        AstType::ForStatement,
        AstType::ConditionalExpression,
    ];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let test = match node.kind() {
            AstKind::IfStatement(statement) => &statement.test,
            AstKind::WhileStatement(statement) => &statement.test,
            AstKind::DoWhileStatement(statement) => &statement.test,
            AstKind::ForStatement(statement) => match &statement.test {
                Some(test) => test,
                None => return,
            },
            AstKind::ConditionalExpression(conditional) => &conditional.test,
            _ => return,
        };
        let count = clauses(test);
        if count > self.max {
            cx.report(test.span()).arg("count", count);
        }
    }
}

/// The checks `&&` and `||` join, also under `!` and parentheses: `a && !(b || c)` has three.
/// An operand of `??` picks a value and is one check with it.
fn clauses(expression: &Expression<'_>) -> u32 {
    match expression.get_inner_expression() {
        Expression::LogicalExpression(logical) if logical.operator != LogicalOperator::Coalesce => {
            clauses(&logical.left) + clauses(&logical.right)
        }
        Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::LogicalNot => {
            clauses(&unary.argument)
        }
        _ => 1,
    }
}
