use kit::oxc::ast::ast::{Decorator, Expression, IdentifierReference, MethodDefinitionKind};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

/// The route decorators of `@nestjs/common` and the status the fix writes for them: the one Nest
/// already answers with. Only GET and POST get it; for the others `204` or `202` is often meant,
/// and the code is a decision, not a default to copy. `Sse` is left out: a stream has no status
/// to declare.
const ROUTES: [(&str, Option<&str>); 9] = [
    ("Get", Some("OK")),
    ("Post", Some("CREATED")),
    ("Put", None),
    ("Patch", None),
    ("Delete", None),
    ("Options", None),
    ("Head", None),
    ("All", None),
    ("Search", None),
];

const NEST: &str = "@nestjs/common";

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::MethodDefinition];
    // `add_import` places the line by `import_kind`.
    const USES: &'static [Fact] = &[Fact::Packages, Fact::Aliases];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::MethodDefinition(method) = node.kind() else {
            return;
        };
        // Nest routes the methods of the prototype only.
        if method.kind != MethodDefinitionKind::Method || method.r#static {
            return;
        }
        let file = cx.file();
        let names: Vec<Option<&str>> = method
            .decorators
            .iter()
            .map(|decorator| called(decorator).and_then(|callee| imported_from(file, callee, NEST)))
            .collect();
        // `@Redirect` answers with a status of its own.
        if names
            .iter()
            .flatten()
            .any(|name| matches!(*name, "HttpCode" | "Redirect"))
        {
            return;
        }
        let mut routes = names.iter().enumerate().filter_map(|(index, name)| {
            let (_, status) = ROUTES.iter().find(|(route, _)| Some(*route) == *name)?;
            Some((index, *status))
        });
        let Some((first, status)) = routes.next() else {
            return;
        };
        // `@Get() @Post()` answers each method with its own default; one code cannot keep both.
        let status = routes.fold(status, |a, (_, b)| if a == b { a } else { None });
        // Decorators apply from the bottom up, and the last `HttpCode` to apply wins. Under the
        // route decorator the new one applies after those below it, so only Nest's own may stand
        // there; else it goes last, applies first, and a `HttpCode` hidden in a decorator of the
        // project still overrides it.
        let anchor = if method.decorators[first + 1..].iter().all(|decorator| {
            called(decorator)
                .and_then(|callee| imported_name(file, callee))
                .is_some_and(|(module, _)| module.starts_with("@nestjs/"))
        }) {
            first
        } else {
            method.decorators.len() - 1
        };
        let anchor = &method.decorators[anchor];
        let fix = status
            .and_then(|status| fix(file, cx.facts(), anchor, status))
            .unwrap_or_default();
        cx.report(method.decorators[first].span)
            .arg(
                "handler",
                one_line(method.key.span().source_text(file.text)),
            )
            .fix(fix);
    }
}

/// The name a decorator calls: `Get` of `@Get(':id')`.
fn called<'d, 'a>(decorator: &'d Decorator<'a>) -> Option<&'d IdentifierReference<'a>> {
    let Expression::CallExpression(call) = &decorator.expression else {
        return None;
    };
    let Expression::Identifier(callee) = &call.callee else {
        return None;
    };
    Some(callee)
}

/// `@HttpCode(HttpStatus.<status>)` on a line of its own under `anchor`, or after it on its line
/// when code follows there, and the import of both names.
fn fix(
    file: &File<'_>,
    facts: &dyn kit::Facts,
    anchor: &Decorator<'_>,
    status: &str,
) -> Option<Vec<Edit>> {
    let mut edits = add_import(file, facts, NEST, &["HttpCode", "HttpStatus"])?;
    let decorator = format!("@HttpCode(HttpStatus.{status})");
    let text = file.text;
    edits.push(match line_tail(file, anchor.span.end) {
        Some((end, _)) => {
            let line =
                &text[line_start(text, anchor.span.start) as usize..anchor.span.start as usize];
            let indent = &line[..line.len() - line.trim_start().len()];
            Edit {
                span: Span::empty(end),
                text: format!("\n{indent}{decorator}"),
            }
        }
        None => Edit {
            span: Span::empty(anchor.span.end),
            text: format!(" {decorator}"),
        },
    });
    Some(edits)
}
