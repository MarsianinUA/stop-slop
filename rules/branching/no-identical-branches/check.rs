use kit::oxc::ast::ast::{
    Argument, BinaryOperator, Expression, IfStatement, Statement, SwitchCase, UnaryOperator,
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
                let Some(second) = repeated_by(file, statement) else {
                    return;
                };
                // A run of `else if`s with one body gets one finding, on its first `if`.
                if let AstKind::IfStatement(parent) = file.semantic.nodes().parent_kind(node.id())
                    && parent
                        .alternate
                        .as_ref()
                        .is_some_and(|alternate| alternate.span() == statement.span)
                    && repeated_by(file, parent).is_some()
                {
                    return;
                }
                let (branches, fix) = match statement.alternate {
                    Some(Statement::IfStatement(_)) => (
                        "This `if` and the `else if` after it",
                        "Join the two conditions with `||` over one copy.",
                    ),
                    _ => (
                        "This `if` and its `else`",
                        "Keep one copy without the `if`; if the condition calls something that must run, keep the call.",
                    ),
                };
                cx.report(head(file, statement.span, statement.consequent.span()))
                    .arg("branches", branches)
                    .arg("line", line(file, second.span()))
                    .arg("fix", fix);
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
                // Labels without a body share the body of the label after them:
                // `case 'b': case 'c': return 1;`.
                let groups: Vec<_> = switch
                    .cases
                    .split_inclusive(|case| !unwrap(&case.consequent).is_empty())
                    .collect();
                // One finding for a run of groups with one body, on its first group.
                let mut in_run = false;
                for pair in groups.windows(2) {
                    let [labels @ [.., case], next @ [first_next, ..]] = pair else {
                        continue;
                    };
                    let body = unwrap(&case.consequent);
                    let next_body = next.last().map_or(&[][..], |last| unwrap(&last.consequent));
                    // Without its label: a `case typeof value === 'string'` reads `value` itself.
                    let branch = Span::new(label_end(case), case.span.end);
                    let mut tests = labels.iter().chain(next.iter());
                    // A body that falls through runs the next one too, so it does not repeat it.
                    let repeated = body.last().is_some_and(Statement::is_jump_statement)
                        && same(body, next_body)
                        && !is_read_in(file, &switch.discriminant, branch)
                        && !narrows(file, &switch.discriminant, branch)
                        && !tests.any(|label| {
                            label
                                .test
                                .as_ref()
                                .is_some_and(|test| narrows(file, test, branch))
                        });
                    if repeated && !in_run {
                        report_cases(cx, case, first_next);
                    }
                    in_run = repeated;
                }
            }
            _ => {}
        }
    }
}

fn report_cases(cx: &mut Cx<'_, '_>, case: &SwitchCase<'_>, next: &SwitchCase<'_>) {
    let file = cx.file();
    let (first, second) = (label(file, case), label(file, next));
    let fix = match (&case.test, &next.test) {
        (None, Some(test)) if is_plain(test) => {
            format!("Drop `{second}`: `default` runs the same code.")
        }
        (Some(test), None) if is_plain(test) => {
            format!("Drop `{first}`: `default` runs the same code.")
        }
        _ => format!("List both labels over one body: `{first}: {second}:`."),
    };
    cx.report(Span::new(case.span.start, label_end(case)))
        .arg("branches", format!("`{first}` and `{second}`"))
        .arg("line", line(file, next.span))
        .arg("fix", fix);
}

fn label(file: &File<'_>, case: &SwitchCase<'_>) -> String {
    match &case.test {
        Some(test) => format!("case {}", one_line(test.span().source_text(file.text))),
        None => "default".to_string(),
    }
}

/// A label that runs nothing when it is tested, so dropping it drops no call: `'paid'`,
/// `Status.Paid`.
fn is_plain(test: &Expression<'_>) -> bool {
    match test.get_inner_expression() {
        Expression::Identifier(_) => true,
        Expression::StaticMemberExpression(member) => is_plain(&member.object),
        test => is_literal_value(test),
    }
}

fn label_end(case: &SwitchCase<'_>) -> u32 {
    case.test
        .as_ref()
        .map_or(case.span.start + "default".len() as u32, |test| {
            test.span().end
        })
}

/// The branch after the `if` (its `else`, or the body of its `else if`) when it runs the same
/// code as the `if` and no test narrows a name they read.
fn repeated_by<'b, 'a>(
    file: &File<'_>,
    statement: &'b IfStatement<'a>,
) -> Option<&'b Statement<'a>> {
    let (second, second_test) = match statement.alternate.as_ref()? {
        Statement::IfStatement(next) => (&next.consequent, Some(&next.test)),
        alternate => (alternate, None),
    };
    let first = statement.consequent.span();
    let narrowed = [Some(&statement.test), second_test]
        .into_iter()
        .flatten()
        .any(|test| narrows(file, test, first));
    (same(body(&statement.consequent), body(second)) && !narrowed).then_some(second)
}

fn body<'b, 'a>(statement: &'b Statement<'a>) -> &'b [Statement<'a>] {
    unwrap(std::slice::from_ref(statement))
}

/// The statements a branch runs: those of its block, of `{ { .. } }` too; none for `;`.
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

/// The test narrows the type of a name the branch reads: `typeof value`, `value instanceof Date`,
/// `'id' in value`, `Array.isArray(value)`, `kind === 'text'`. Each branch may then pick another
/// overload with the same call (`typeof v === 'string' ? format(v) : format(v)`) or write a key
/// of its own type (`to[kind] = from[kind]`), so one copy would not compile.
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
            operator => [&binary.left, &binary.right].into_iter().any(|side| {
                narrows(file, side, branch)
                    || (operator.is_equality() && is_read_in(file, side, branch))
            }),
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

/// Of a member path (`order.total`, `this.value`), TypeScript narrows the path, so its root
/// stands for it.
fn is_read_in(file: &File<'_>, expression: &Expression<'_>, branch: Span) -> bool {
    let mut expression = expression.get_inner_expression();
    loop {
        let member = match expression {
            Expression::ChainExpression(chain) => chain.expression.as_member_expression(),
            expression => expression.as_member_expression(),
        };
        let Some(member) = member else {
            break;
        };
        expression = member.object().get_inner_expression();
    }
    let nodes = file.semantic.nodes();
    let id = match expression {
        Expression::Identifier(id) => id,
        Expression::ThisExpression(_) => {
            return nodes.iter().any(|node| {
                matches!(node.kind(), AstKind::ThisExpression(_))
                    && branch.contains_inclusive(node.span())
            });
        }
        _ => return false,
    };
    let scoping = file.semantic.scoping();
    let Some(symbol) = scoping.get_reference(id.reference_id()).symbol_id() else {
        return false;
    };
    scoping
        .get_resolved_references(symbol)
        .any(|reference| branch.contains_inclusive(nodes.get_node(reference.node_id()).span()))
}

fn line(file: &File<'_>, span: Span) -> u32 {
    file.lines.position(span.start).line
}

/// `if (order.isPaid)` of `if (order.isPaid) {`: from the keyword to the body.
fn head(file: &File<'_>, statement: Span, body: Span) -> Span {
    let text = Span::new(statement.start, body.start).source_text(file.text);
    Span::sized(statement.start, text.trim_end().len() as u32)
}
