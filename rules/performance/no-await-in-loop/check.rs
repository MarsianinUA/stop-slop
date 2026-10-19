use kit::oxc::ast::ast::{Expression, Statement};
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
        let Some(around) = loop_around(file, node.id()) else {
            return;
        };
        // The test and the update decide whether another pass runs, so they wait by nature;
        // `for await` reads one item at a time on purpose; an `await` in a callback holds up
        // only that callback.
        let Some(body) = body(around.node.kind()).map(GetSpan::span) else {
            return;
        };
        if !body.contains_inclusive(awaited.span) {
            return;
        }
        cx.report(awaited.span)
            .arg("call", awaited_text(file, &awaited.argument))
            .arg("receiver", receiver(file, &awaited.argument))
            .arg("writes", writes(file, around.node.span(), body))
            .arg("exit", exit(file, body));
    }
}

/// The body of a loop statement other than `for await`.
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

/// The object the awaited call goes to, or the function it calls, and whether its name says
/// store or client: `this.ordersRepository` of `this.ordersRepository.save(..)`, `fetchPage` of
/// `fetchPage(cursor)`.
fn receiver(file: &File<'_>, argument: &Expression<'_>) -> String {
    let Some(call) = call_of(argument) else {
        return "none, the awaited value is not a call".to_string();
    };
    let callee = call.callee.get_inner_expression();
    let target = callee
        .get_member_expr()
        .map_or(callee, |member| member.object());
    let text = one_line(target.span().source_text(file.text));
    if is_round_trip(target) {
        format!("`{text}`, a store or a client by its name")
    } else {
        format!("`{text}`, not a store or a client by its name")
    }
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

/// The variables declared outside the loop that its body assigns, in order: `cursor` of
/// `cursor = page.next`. A pass that writes one may need the pass before it.
fn writes(file: &File<'_>, loop_span: Span, body: Span) -> String {
    let scoping = file.semantic.scoping();
    let mut names: Vec<&str> = Vec::new();
    for node in file.semantic.nodes().iter() {
        let AstKind::IdentifierReference(id) = node.kind() else {
            continue;
        };
        let reference = scoping.get_reference(id.reference_id());
        let Some(symbol) = reference.symbol_id() else {
            continue;
        };
        let name = scoping.symbol_name(symbol);
        if reference.is_write()
            && body.contains_inclusive(id.span)
            && !loop_span.contains_inclusive(scoping.symbol_span(symbol))
            && !names.contains(&name)
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

/// How the body leaves the loop early. A `break` of a loop or `switch` inside the body and a
/// `return` of a function inside it stay in the body.
fn exit(file: &File<'_>, body: Span) -> &'static str {
    let nodes = file.semantic.nodes();
    let mut breaks = false;
    let mut returns = false;
    for node in nodes
        .iter()
        .filter(|node| body.contains_inclusive(node.span()))
    {
        match node.kind() {
            AstKind::BreakStatement(statement) => {
                let target = nodes.ancestors(node.id()).find(|ancestor| {
                    match (&statement.label, ancestor.kind()) {
                        (Some(label), AstKind::LabeledStatement(labeled)) => {
                            labeled.label.name == label.name
                        }
                        (None, kind) => is_breakable(kind),
                        _ => false,
                    }
                });
                breaks |= is_outside(body, target);
            }
            AstKind::ReturnStatement(_) => {
                let function = nodes
                    .ancestors(node.id())
                    .find(|ancestor| ancestor.kind().is_function_like());
                returns |= is_outside(body, function);
            }
            _ => {}
        }
    }
    match (breaks, returns) {
        (true, true) => "`break` and `return`",
        (true, false) => "`break`",
        (false, true) => "`return`",
        (false, false) => "none",
    }
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

/// The `break` or `return` reaches past the body: its target is the loop or something around it.
fn is_outside(body: Span, target: Option<&AstNode<'_>>) -> bool {
    target.is_none_or(|target| !body.contains_inclusive(target.span()))
}
