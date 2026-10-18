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
        let Some(argument) = call.arguments.first().and_then(Argument::as_expression) else {
            return;
        };
        let file = cx.file();
        let is_route = imported_from(file, callee, "@nestjs/common")
            .is_some_and(|name| ROUTES.contains(&name));
        if !is_route {
            return;
        }
        let paths: Vec<Span> = match argument {
            // `@Controller({ path: 'orders', version: '1' })`.
            Expression::ObjectExpression(options) => options
                .properties
                .iter()
                .filter_map(|property| match property {
                    ObjectPropertyKind::ObjectProperty(property)
                        if property.key.is_specific_static_name("path") =>
                    {
                        Some(&property.value)
                    }
                    _ => None,
                })
                .flat_map(path_literals)
                .collect(),
            argument => path_literals(argument),
        };
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
fn path_literals(value: &Expression<'_>) -> Vec<Span> {
    match value {
        Expression::ArrayExpression(array) => array
            .elements
            .iter()
            .filter_map(ArrayExpressionElement::as_expression)
            .filter_map(path_literal)
            .collect(),
        value => path_literal(value).into_iter().collect(),
    }
}

fn path_literal(value: &Expression<'_>) -> Option<Span> {
    match value {
        Expression::StringLiteral(literal) => Some(literal.span),
        Expression::TemplateLiteral(template) if template.is_no_substitution_template() => {
            Some(template.span)
        }
        _ => None,
    }
}

/// A segment with other characters is route syntax and passes: `:id`, `*path`, `{/:id}`,
/// `(.*)`. Parts between dots are each a word (`robots.txt`, `llms-all.txt`); an empty part has
/// no case to judge (`.well-known`, `.`, `..`).
fn passes(segment: &str) -> bool {
    !segment
        .chars()
        .all(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        || segment
            .split('.')
            .all(|part| part.is_empty() || Case::Kebab.matches(part))
}

/// `userSettings` -> `user-settings`, `Robots.TXT` -> `robots.txt`.
fn kebab(segment: &str) -> String {
    let parts: Vec<String> = segment
        .split('.')
        .map(|part| {
            Case::Kebab
                .apply(part)
                .unwrap_or_else(|| part.to_lowercase())
        })
        .collect();
    parts.join(".")
}
