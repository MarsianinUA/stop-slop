use kit::oxc::ast::ast::{Argument, ArrayExpressionElement, Expression, ObjectPropertyKind};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

/// The decorators of `@nestjs/common` whose first argument is a path.
const ROUTES: [&str; 11] = [
    "Controller",
    "Get",
    "Post",
    "Put",
    "Patch",
    "Delete",
    "Options",
    "Head",
    "All",
    "Search",
    "Sse",
];

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::Decorator];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::Decorator(decorator) = node.kind() else {
            return;
        };
        let Expression::CallExpression(call) = &decorator.expression else {
            return;
        };
        let Expression::Identifier(callee) = &call.callee else {
            return;
        };
        let file = cx.file();
        let is_route = matches!(imported_name(file, callee),
            Some((module, name)) if package_name(module) == "@nestjs/common" && ROUTES.contains(&name));
        if !is_route {
            return;
        }
        let mut paths = Vec::new();
        if let Some(argument) = call.arguments.first() {
            match argument {
                // `@Controller({ path: 'orders', version: '1' })`.
                Argument::ObjectExpression(options) => {
                    for property in &options.properties {
                        if let ObjectPropertyKind::ObjectProperty(property) = property
                            && property.key.is_specific_static_name("path")
                        {
                            path_literals(&property.value, &mut paths);
                        }
                    }
                }
                argument => {
                    if let Some(argument) = argument.as_expression() {
                        path_literals(argument, &mut paths);
                    }
                }
            }
        }
        for literal in paths {
            // Between the quotes, as written: offsets stay those of the file.
            let start = literal.start + 1;
            let raw = &file.text[start as usize..literal.end as usize - 1];
            let mut at = start;
            for segment in raw.split('/') {
                if !passes(segment) {
                    cx.report(Span::sized(at, segment.len() as u32))
                        .arg("segment", segment)
                        .arg("kebab", kebab(segment));
                }
                at += segment.len() as u32 + 1;
            }
        }
    }
}

/// The spans of the literals that hold paths: a string, a template without `${}`, or an array
/// of them.
fn path_literals(value: &Expression<'_>, out: &mut Vec<Span>) {
    match value {
        Expression::StringLiteral(literal) => out.push(literal.span),
        Expression::TemplateLiteral(template) if template.expressions.is_empty() => {
            out.push(template.span);
        }
        Expression::ArrayExpression(array) => {
            for element in &array.elements {
                if let ArrayExpressionElement::StringLiteral(literal) = element {
                    out.push(literal.span);
                } else if let ArrayExpressionElement::TemplateLiteral(template) = element
                    && template.expressions.is_empty()
                {
                    out.push(template.span);
                }
            }
        }
        _ => {}
    }
}

/// A segment with other characters is route syntax and passes: `:id`, `*path`, `{/:id}`,
/// `(.*)`. One leading `.` (`.well-known`) is dropped; parts between dots are each a word
/// (`robots.txt`, `llms-all.txt`).
fn passes(segment: &str) -> bool {
    if segment.is_empty()
        || !segment
            .chars()
            .all(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return true;
    }
    let segment = segment.strip_prefix('.').unwrap_or(segment);
    segment.split('.').all(|part| Case::Kebab.matches(part))
}

/// `userSettings` -> `user-settings`, `Robots.TXT` -> `robots.txt`.
fn kebab(segment: &str) -> String {
    let (dot, rest) = match segment.strip_prefix('.') {
        Some(rest) => (".", rest),
        None => ("", segment),
    };
    let parts: Vec<String> = rest
        .split('.')
        .map(|part| {
            if Case::Kebab.matches(part) {
                part.to_string()
            } else {
                Case::Kebab
                    .apply(part)
                    .unwrap_or_else(|| part.to_lowercase())
            }
        })
        .collect();
    format!("{dot}{}", parts.join("."))
}
