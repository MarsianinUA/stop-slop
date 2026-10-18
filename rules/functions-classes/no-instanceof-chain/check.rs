use kit::oxc::ast::ast::{BinaryOperator, Expression, LogicalOperator};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[
        AstType::Program,
        AstType::FunctionBody,
        AstType::BlockStatement,
        AstType::StaticBlock,
        AstType::SwitchCase,
        AstType::TSModuleBlock,
        AstType::ConditionalExpression,
    ];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        match node.kind() {
            AstKind::ConditionalExpression(conditional) => {
                if is_inner_ternary(cx.file(), node) {
                    return;
                }
                let mut tests = vec![&conditional.test];
                let mut alternate = &conditional.alternate;
                while let Expression::ConditionalExpression(next) = alternate.get_inner_expression()
                {
                    tests.push(&next.test);
                    alternate = &next.alternate;
                }
                report_chains(cx, &tests);
            }
            kind => {
                for tests in if_runs(kind) {
                    report_chains(cx, &tests);
                }
            }
        }
    }
}

/// The `alternate` of another `?:`: its chain is checked from the head.
fn is_inner_ternary(file: &File<'_>, node: &AstNode<'_>) -> bool {
    let outer = outer_assertion(file, node);
    matches!(file.semantic.nodes().parent_kind(outer.id()), AstKind::ConditionalExpression(parent)
        if parent.alternate.span() == outer.span())
}

struct Checked<'a> {
    name: &'a str,
    first: Span,
    branches: u32,
    classes: Vec<&'a str>,
}

fn report_chains<'a>(cx: &mut Cx<'_, 'a>, tests: &[&'a Expression<'a>]) {
    let file = cx.file();
    let mut values: Vec<Checked<'a>> = Vec::new();
    for test in tests {
        let mut checks = Vec::new();
        instanceof_checks(file, test, &mut checks);
        let mut counted: Vec<&str> = Vec::new();
        for (name, class, span) in checks {
            let index = values
                .iter()
                .position(|value| value.name == name)
                .unwrap_or_else(|| {
                    values.push(Checked {
                        name,
                        first: span,
                        branches: 0,
                        classes: Vec::new(),
                    });
                    values.len() - 1
                });
            let value = &mut values[index];
            if !counted.contains(&name) {
                counted.push(name);
                value.branches += 1;
            }
            if !value.classes.contains(&class) {
                value.classes.push(class);
            }
        }
    }
    // One class in every branch (`e instanceof HttpError && e.status === 404`) is no chain of
    // classes.
    for value in values
        .into_iter()
        .filter(|value| value.branches >= 2 && value.classes.len() >= 2)
    {
        cx.report(value.first)
            .arg("name", value.name)
            .arg("classes", value.classes.join("`, `"));
    }
}

/// `(value, class, span)` of each `value instanceof Class` in a test, also inside `&&` / `||`.
/// The span of the check, not of the whole test: two values in one test get two places.
fn instanceof_checks<'a>(
    file: &File<'a>,
    test: &'a Expression<'a>,
    out: &mut Vec<(&'a str, &'a str, Span)>,
) {
    match test.get_inner_expression() {
        Expression::BinaryExpression(binary) if binary.operator == BinaryOperator::Instanceof => {
            let text = |expression: &Expression<'_>| {
                expression
                    .get_inner_expression()
                    .span()
                    .source_text(file.text)
            };
            out.push((text(&binary.left), text(&binary.right), binary.span));
        }
        Expression::LogicalExpression(logical) if logical.operator != LogicalOperator::Coalesce => {
            instanceof_checks(file, &logical.left, out);
            instanceof_checks(file, &logical.right, out);
        }
        _ => {}
    }
}
