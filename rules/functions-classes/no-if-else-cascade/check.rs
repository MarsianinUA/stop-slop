use kit::oxc::ast::ast::{BinaryOperator, Expression, LogicalOperator, UnaryOperator};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    min: u32,
}

impl Rule for Check {
    const NODES: &'static [AstType] = &[
        AstType::Program,
        AstType::FunctionBody,
        AstType::BlockStatement,
        AstType::StaticBlock,
        AstType::SwitchCase,
        AstType::TSModuleBlock,
    ];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        for tests in if_runs(node.kind()) {
            let mut values: Vec<Compared<'a>> = Vec::new();
            for test in tests {
                let mut compared = Vec::new();
                compared_values(file, test, &mut compared);
                for (name, span) in compared {
                    match values.iter_mut().find(|value| value.name == name) {
                        Some(value) => value.branches += 1,
                        None => values.push(Compared {
                            name,
                            first: span,
                            branches: 1,
                        }),
                    }
                }
            }
            for value in values {
                if value.branches >= self.min {
                    cx.report(value.first)
                        .arg("name", value.name)
                        .arg("count", value.branches);
                }
            }
        }
    }
}

struct Compared<'a> {
    name: &'a str,
    first: Span,
    branches: u32,
}

/// The values a test compares with `===` / `==` to a constant, with the span of the first such
/// comparison: `status === Status.Open`, `'mg' === unit`, each side of
/// `unit === 'g' || unit === 'mg'`. The span of the comparison, not of the whole test: two values
/// in one test get two places.
fn compared_values<'a>(file: &File<'a>, test: &'a Expression<'a>, out: &mut Vec<(&'a str, Span)>) {
    match test.get_inner_expression() {
        Expression::BinaryExpression(binary)
            if matches!(
                binary.operator,
                BinaryOperator::StrictEquality | BinaryOperator::Equality
            ) =>
        {
            let (left, right) = (
                binary.left.get_inner_expression(),
                binary.right.get_inner_expression(),
            );
            if is_null_or_boolean(file, left) || is_null_or_boolean(file, right) {
                return;
            }
            // A literal is never the value; of two members (`order.kind === Kind.Gift`), the left.
            let value = match (constant(left), constant(right)) {
                (Some(Constant::Literal), Some(Constant::Literal)) => return,
                (Some(Constant::Literal), _) | (Some(Constant::Member), None) => right,
                (_, Some(_)) => left,
                (None, None) => return,
            };
            let text = value.span().source_text(file.text);
            if !out.iter().any(|(name, _)| *name == text) {
                out.push((text, binary.span));
            }
        }
        Expression::LogicalExpression(logical) if logical.operator == LogicalOperator::Or => {
            compared_values(file, &logical.left, out);
            compared_values(file, &logical.right, out);
        }
        _ => {}
    }
}

fn is_null_or_boolean(file: &File<'_>, expression: &Expression<'_>) -> bool {
    matches!(
        expression,
        Expression::NullLiteral(_) | Expression::BooleanLiteral(_)
    ) || is_global(file, expression, "undefined")
}

enum Constant {
    /// A string, number (also `-1`), bigint, a template without `${}`.
    Literal,
    /// A member of a name: `Status.Open`.
    Member,
}

fn constant(expression: &Expression<'_>) -> Option<Constant> {
    match expression {
        Expression::StringLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BigIntLiteral(_) => Some(Constant::Literal),
        Expression::TemplateLiteral(template) if template.expressions.is_empty() => {
            Some(Constant::Literal)
        }
        Expression::UnaryExpression(unary)
            if matches!(
                unary.operator,
                UnaryOperator::UnaryNegation | UnaryOperator::UnaryPlus
            ) && matches!(
                unary.argument.get_inner_expression(),
                Expression::NumericLiteral(_) | Expression::BigIntLiteral(_)
            ) =>
        {
            Some(Constant::Literal)
        }
        _ => expression
            .get_member_expr()
            .filter(|member| {
                matches!(
                    member.object().get_inner_expression(),
                    Expression::Identifier(_)
                )
            })
            .map(|_| Constant::Member),
    }
}
