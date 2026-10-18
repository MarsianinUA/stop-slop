use kit::oxc::ast::ast::Expression;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::SpreadElement];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::SpreadElement(spread) = node.kind() else {
            return;
        };
        let Expression::ConditionalExpression(conditional) = spread.argument.get_inner_expression()
        else {
            return;
        };
        if is_empty_object(&conditional.consequent) || is_empty_object(&conditional.alternate) {
            cx.report(spread.span);
        }
    }
}

fn is_empty_object(expression: &Expression<'_>) -> bool {
    matches!(expression.get_inner_expression(),
        Expression::ObjectExpression(object) if object.properties.is_empty())
}
