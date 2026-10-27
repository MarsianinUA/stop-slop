use kit::oxc::ast::ast::{Argument, Expression, LabelIdentifier, Statement};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

/// Words of a name that make each call a round trip to a store or a service.
const ROUND_TRIPS: &[&str] = &[
    "repository",
    "repo",
    "db",
    "database",
    "prisma",
    "knex",
    "orm",
    "sql",
    "mongo",
    "redis",
    "cache",
    "client",
    "http",
    "api",
    "fetch",
    "axios",
    "gateway",
    "pool",
    "connection",
    "queue",
    "store",
    "bucket",
];

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::AwaitExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::AwaitExpression(awaited) = node.kind() else {
            return;
        };
        let file = cx.file();
        // A pass that starts a batch together, such as the chunk the message suggests. A fixed
        // list, `Promise.all([load(user), save(user)])`, still waits once per item.
        if call_of(&awaited.argument).is_some_and(|call| {
            matches!(
                global_member(file, &call.callee),
                Some(("Promise", "all" | "allSettled"))
            ) && !matches!(
                call.arguments
                    .first()
                    .and_then(Argument::as_expression)
                    .map(Expression::get_inner_expression),
                Some(Expression::ArrayExpression(_))
            )
        }) {
            return;
        }
        let mut around = loop_around(file, node.id());
        let (around, body) = loop {
            let Some(current) = around else {
                return;
            };
            // `for await` reads one item at a time on purpose; an `await` in a callback holds up
            // only that callback.
            let Some(body) = body(current.node.kind()).map(GetSpan::span) else {
                return;
            };
            if body.contains_inclusive(awaited.span) {
                break (current, body);
            }
            // The test and the update decide whether another pass runs, so they wait by nature.
            // The loop around this one holds them in its body.
            around = loop_around(file, current.node.id());
        };
        cx.report(awaited.span)
            .arg("call", awaited_text(file, &awaited.argument))
            .arg("receiver", receiver(file, &awaited.argument))
            .arg("writes", writes(file, &around, body))
            .arg("exit", exit(file, around.node.span(), body));
    }
}

fn body<'a>(kind: AstKind<'a>) -> Option<&'a Statement<'a>> {
    match kind {
        AstKind::ForStatement(statement) => Some(&statement.body),
        AstKind::ForOfStatement(statement) if !statement.r#await => Some(&statement.body),
        AstKind::ForInStatement(statement) => Some(&statement.body),
        AstKind::WhileStatement(statement) => Some(&statement.body),
        AstKind::DoWhileStatement(statement) => Some(&statement.body),
        _ => None,
    }
}

/// `this.orders.save(..)` for an awaited call, else the awaited source.
fn awaited_text(file: &File<'_>, argument: &Expression<'_>) -> String {
    match call_of(argument) {
        Some(call) => format!(
            "{}(..)",
            one_line(call.callee.span().source_text(file.text))
        ),
        None => one_line(argument.span().source_text(file.text)),
    }
}

/// The object the awaited call goes to, or the function it calls, and whether its name says the
/// call is a round trip: `this.ordersRepository` of `this.ordersRepository.save(..)`, `fetchPage`
/// of `fetchPage(cursor)`.
fn receiver(file: &File<'_>, argument: &Expression<'_>) -> String {
    let Some(call) = call_of(argument) else {
        return "none, the awaited value is not a call".to_string();
    };
    let callee = call.callee.get_inner_expression();
    let target = callee
        .get_member_expr()
        .map_or(callee, |member| member.object());
    let text = one_line(target.span().source_text(file.text));
    let looks = if is_round_trip(target) {
        "looks"
    } else {
        "does not look"
    };
    format!("`{text}`, which by its name {looks} like a round trip to a store or a service")
}

/// A name along the chain has a word of `ROUND_TRIPS`: `prisma` of `this.prisma.user`, `http` of
/// `this.http.get(url)`.
fn is_round_trip(expression: &Expression<'_>) -> bool {
    let has_word = |name: &str| {
        words(name).iter().any(|word| {
            ROUND_TRIPS
                .iter()
                .any(|trip| trip.eq_ignore_ascii_case(word))
        })
    };
    let mut current = expression.get_inner_expression();
    loop {
        let name = match current {
            Expression::Identifier(id) => return has_word(id.name.as_str()),
            Expression::StaticMemberExpression(member) => {
                current = member.object.get_inner_expression();
                member.property.name.as_str()
            }
            Expression::PrivateFieldExpression(member) => {
                current = member.object.get_inner_expression();
                member.field.name.as_str()
            }
            Expression::CallExpression(call) => {
                current = call.callee.get_inner_expression();
                continue;
            }
            _ => return false,
        };
        if has_word(name) {
            return true;
        }
    }
}

/// The variables the body assigns that carry over from pass to pass, in order: `cursor` of
/// `cursor = page.next`, declared before the loop or in the `init` of `for`. A pass that writes
/// one may need the pass before it.
fn writes(file: &File<'_>, around: &LoopAround<'_>, body: Span) -> String {
    let scoping = file.semantic.scoping();
    let nodes = file.semantic.nodes();
    let mut names: Vec<&str> = Vec::new();
    for node in nodes.iter() {
        let AstKind::IdentifierReference(id) = node.kind() else {
            continue;
        };
        let reference = scoping.get_reference(id.reference_id());
        let Some(symbol) = reference.symbol_id() else {
            continue;
        };
        let name = scoping.symbol_name(symbol);
        let declaration = scoping.symbol_declaration(symbol);
        // `runs_each_pass` stops at a function that is not a per-item callback, such as a `sort`
        // comparator, but its locals start anew on each call too.
        if reference.is_write()
            && body.contains_inclusive(id.span)
            && !names.contains(&name)
            && !body.contains_inclusive(nodes.get_node(declaration).span())
            && !around.runs_each_pass(file, declaration)
        {
            names.push(name);
        }
    }
    if names.is_empty() {
        return "none".to_string();
    }
    names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// How the body leaves the loop early: `break`, `continue` to an outer loop, `return`, or a
/// `throw` that no `catch` in the body takes. One that ends at a loop, `switch`, function or
/// `try` inside the body stays in the body.
fn exit(file: &File<'_>, loop_span: Span, body: Span) -> String {
    let nodes = file.semantic.nodes();
    let mut exits: Vec<&str> = Vec::new();
    for node in nodes
        .iter()
        .filter(|node| body.contains_inclusive(node.span()))
    {
        let mut ancestors = nodes.ancestors(node.id());
        let (exit, target) = match node.kind() {
            AstKind::BreakStatement(statement) => (
                "`break`",
                ancestors.find(|ancestor| match &statement.label {
                    Some(label) => is_labeled(ancestor, label),
                    None => is_breakable(ancestor.kind()),
                }),
            ),
            AstKind::ContinueStatement(statement) => {
                // An unlabeled `continue`, or one to the label of this loop, starts its next pass.
                let Some(label) = &statement.label else {
                    continue;
                };
                let target = ancestors.find(|ancestor| is_labeled(ancestor, label));
                if target.is_some_and(|target| {
                    matches!(target.kind(), AstKind::LabeledStatement(labeled)
                        if labeled.body.span() == loop_span)
                }) {
                    continue;
                }
                ("`continue`", target)
            }
            AstKind::ReturnStatement(_) => (
                "`return`",
                ancestors.find(|ancestor| ancestor.kind().is_function_like()),
            ),
            AstKind::ThrowStatement(statement) => (
                "`throw`",
                ancestors.find(|ancestor| match ancestor.kind() {
                    AstKind::TryStatement(caught) => {
                        caught.handler.is_some()
                            && caught.block.span.contains_inclusive(statement.span)
                    }
                    kind => kind.is_function_like(),
                }),
            ),
            _ => continue,
        };
        if is_outside(body, target) && !exits.contains(&exit) {
            exits.push(exit);
        }
    }
    if exits.is_empty() {
        return "none".to_string();
    }
    exits.join(", ")
}

fn is_labeled(node: &AstNode<'_>, label: &LabelIdentifier<'_>) -> bool {
    matches!(node.kind(), AstKind::LabeledStatement(labeled) if labeled.label.name == label.name)
}

fn is_breakable(kind: AstKind<'_>) -> bool {
    matches!(
        kind,
        AstKind::ForStatement(_)
            | AstKind::ForInStatement(_)
            | AstKind::ForOfStatement(_)
            | AstKind::WhileStatement(_)
            | AstKind::DoWhileStatement(_)
            | AstKind::SwitchStatement(_)
    )
}

/// The exit reaches past the body: its target is the loop or something around it.
fn is_outside(body: Span, target: Option<&AstNode<'_>>) -> bool {
    target.is_none_or(|target| !body.contains_inclusive(target.span()))
}
