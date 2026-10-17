use kit::oxc::ast::ast::{ChainElement, Expression, Statement};
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
    const NODES: &'static [AstType] = &[AstType::CatchClause];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::CatchClause(clause) = node.kind() else {
            return;
        };
        // A comment in the block explains the silence but does not handle the error.
        let body = &clause.body.body;
        let does = if body.is_empty() {
            "is empty"
        } else if body.iter().all(|statement| self.is_log(statement)) {
            "only logs"
        } else {
            return;
        };
        cx.report(clause.span).arg("does", does);
    }
}

impl Check {
    /// `this.logger.warn(..)`, `console.error(..)`, `logger?.info(..)`, `await log.error(..)`.
    fn is_log(&self, statement: &Statement<'_>) -> bool {
        let Statement::ExpressionStatement(statement) = statement else {
            return false;
        };
        let mut expression = statement.expression.get_inner_expression();
        if let Expression::AwaitExpression(awaited) = expression {
            expression = awaited.argument.get_inner_expression();
        }
        let call = match expression {
            Expression::CallExpression(call) => call,
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::CallExpression(call) => call,
                _ => return false,
            },
            _ => return false,
        };
        let Expression::StaticMemberExpression(method) = call.callee.get_inner_expression() else {
            return false;
        };
        let receiver = match method.object.get_inner_expression() {
            Expression::Identifier(id) => id.name.as_str(),
            Expression::StaticMemberExpression(member) => member.property.name.as_str(),
            Expression::PrivateFieldExpression(member) => member.field.name.as_str(),
            _ => return false,
        };
        self.levels
            .iter()
            .any(|level| level == method.property.name.as_str())
            && words(receiver).iter().any(|word| {
                self.loggers
                    .iter()
                    .any(|logger| logger.eq_ignore_ascii_case(word))
            })
    }
}
