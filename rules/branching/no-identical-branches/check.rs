use kit::oxc::ast::ast::{
    Argument, BinaryOperator, Expression, Statement, SwitchCase, UnaryOperator,
};
use kit::oxc::span::ContentEq;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[
        AstType::IfStatement,
        AstType::ConditionalExpression,
        AstType::SwitchStatement,
    ];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        match node.kind() {
            AstKind::IfStatement(statement) => {
                let Some(alternate) = &statement.alternate else {
                    return;
                };
                let first = statement.consequent.span();
                // Of `if (a) X else if (b) X`, the branch after the `else if`: X runs when `a || b`.
                let (second, branches, fix) = match alternate {
                    Statement::IfStatement(next) if !narrows(file, &next.test, first) => (
                        &next.consequent,
                        "This `if` and the `else if` after it",
                        "Join the two conditions with `||` over one copy.",
                    ),
                    Statement::IfStatement(_) => return,
                    alternate => (
                        alternate,
                        "This `if` and its `else`",
                        "Keep one copy without the `if`; if the condition calls something that must run, keep the call.",
                    ),
                };
                if same(body(&statement.consequent), body(second))
                    && !narrows(file, &statement.test, first)
                {
                    cx.report(head(file, statement.span, first))
                        .arg("branches", branches)
                        .arg("line", line(file, second.span()))
                        .arg("fix", fix);
                }
            }
            AstKind::ConditionalExpression(conditional) => {
                if conditional.consequent.content_eq(&conditional.alternate)
                    && !narrows(file, &conditional.test, conditional.consequent.span())
                {
                    cx.report(conditional.span)
                        .arg("branches", "The two branches of this `?:`")
                        .arg("line", line(file, conditional.alternate.span()))
                        .arg(
                            "fix",
                            "Keep the value without the `?:`; if the condition calls something that must run, keep the call.",
                        );
                }
            }
            AstKind::SwitchStatement(switch) => {
                // One finding for a run of cases with one body, on its first case.
                let mut in_run = false;
                for pair in switch.cases.windows(2) {
                    let [case, next] = pair else {
                        continue;
                    };
                    // A body that falls through runs the next one too, so it does not repeat it.
                    let repeated = ends_in_jump(case)
                        && same(body_of(case), body_of(next))
                        && !narrows(file, &switch.discriminant, case.span);
                    if repeated && !in_run {
                        report_cases(file, cx, case, next);
                    }
                    in_run = repeated;
                }
            }
            _ => {}
        }
    }
}

fn report_cases(
    file: &File<'_>,
    cx: &mut Cx<'_, '_>,
    case: &SwitchCase<'_>,
    next: &SwitchCase<'_>,
) {
    let (first, second) = (label(file, case), label(file, next));
    let fix = match (&case.test, &next.test) {
        (Some(_), Some(_)) => format!("List both labels over one body: `{first}: {second}:`."),
        (None, _) => format!("Drop `{second}`: `default` runs the same code."),
        (_, None) => format!("Drop `{first}`: `default` runs the same code."),
    };
    let end = case
        .test
        .as_ref()
        .map_or(case.span.start + "default".len() as u32, |test| {
            test.span().end
        });
    cx.report(Span::new(case.span.start, end))
        .arg("branches", format!("`{first}` and `{second}`"))
        .arg("line", line(file, next.span))
        .arg("fix", fix);
}

/// `case 'paid'`, `default`.
fn label(file: &File<'_>, case: &SwitchCase<'_>) -> String {
    match &case.test {
        Some(test) => format!("case {}", one_line(test.span().source_text(file.text))),
        None => "default".to_string(),
    }
}

/// The statements a branch runs: those of its block, of `{ { .. } }` too; none for `;`.
fn body<'b, 'a>(statement: &'b Statement<'a>) -> &'b [Statement<'a>] {
    unwrap(std::slice::from_ref(statement))
}

fn body_of<'b, 'a>(case: &'b SwitchCase<'a>) -> &'b [Statement<'a>] {
    unwrap(&case.consequent)
}

fn unwrap<'b, 'a>(mut statements: &'b [Statement<'a>]) -> &'b [Statement<'a>] {
    loop {
        match statements {
            [Statement::BlockStatement(block)] => statements = &block.body,
            [Statement::EmptyStatement(_)] => return &[],
            _ => return statements,
        }
    }
}

/// `ContentEq` compares the trees, so comments, parentheses, quotes and spacing do not count.
/// Two empty branches are left to ESLint's `no-empty`.
fn same(first: &[Statement<'_>], second: &[Statement<'_>]) -> bool {
    !first.is_empty()
        && first.len() == second.len()
        && first.iter().zip(second).all(|(a, b)| a.content_eq(b))
}

fn ends_in_jump(case: &SwitchCase<'_>) -> bool {
    matches!(
        body_of(case).last(),
        Some(
            Statement::BreakStatement(_)
                | Statement::ContinueStatement(_)
                | Statement::ReturnStatement(_)
                | Statement::ThrowStatement(_)
        )
    )
}

/// The test narrows the type of a name the branch reads: `typeof value`, `value instanceof Date`,
/// `'id' in value`, `Array.isArray(value)`. Each branch may then pick another overload with the
/// same call (`typeof v === 'string' ? format(v) : format(v)`), so one copy would not compile.
fn narrows(file: &File<'_>, test: &Expression<'_>, branch: Span) -> bool {
    match test.get_inner_expression() {
        Expression::LogicalExpression(logical) => {
            narrows(file, &logical.left, branch) || narrows(file, &logical.right, branch)
        }
        Expression::UnaryExpression(unary) => match unary.operator {
            UnaryOperator::Typeof => is_read_in(file, &unary.argument, branch),
            UnaryOperator::LogicalNot => narrows(file, &unary.argument, branch),
            _ => false,
        },
        Expression::BinaryExpression(binary) => match binary.operator {
            BinaryOperator::Instanceof => is_read_in(file, &binary.left, branch),
            BinaryOperator::In => is_read_in(file, &binary.right, branch),
            _ => narrows(file, &binary.left, branch) || narrows(file, &binary.right, branch),
        },
        Expression::CallExpression(call)
            if global_member(file, &call.callee) == Some(("Array", "isArray")) =>
        {
            call.arguments
                .first()
                .and_then(Argument::as_expression)
                .is_some_and(|argument| is_read_in(file, argument, branch))
        }
        _ => false,
    }
}

fn is_read_in(file: &File<'_>, expression: &Expression<'_>, branch: Span) -> bool {
    let Expression::Identifier(id) = expression.get_inner_expression() else {
        return false;
    };
    let scoping = file.semantic.scoping();
    let Some(symbol) = scoping.get_reference(id.reference_id()).symbol_id() else {
        return false;
    };
    let nodes = file.semantic.nodes();
    scoping.get_resolved_references(symbol).any(|reference| {
        let span = nodes.get_node(reference.node_id()).span();
        branch.start <= span.start && span.end <= branch.end
    })
}

fn line(file: &File<'_>, span: Span) -> u32 {
    file.lines.position(span.start).line
}

/// `if (order.isPaid)` of `if (order.isPaid) {`: from the keyword to the body.
fn head(file: &File<'_>, statement: Span, body: Span) -> Span {
    let text = Span::new(statement.start, body.start).source_text(file.text);
    Span::sized(statement.start, text.trim_end().len() as u32)
}
