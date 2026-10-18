use kit::oxc::ast::ast::{Argument, Expression, ObjectPropertyKind};
use kit::prelude::*;

/// Words of the class name and the file name that mark a response, and words of the class name
/// that mark an input: the class name decides first (`CreateOrderRequestDto` in
/// `order.response.dto.ts` is an input).
#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    responses: Vec<String>,
    inputs: Vec<String>,
}

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
        let optional = match imported_from(file, callee, "@nestjs/swagger") {
            Some("ApiPropertyOptional") => true,
            // `@ApiPropertyOptional()` is `@ApiProperty({ required: false })`.
            Some("ApiProperty") => call.arguments.first().is_some_and(is_not_required),
            _ => false,
        };
        if !optional {
            return;
        }
        let nodes = file.semantic.nodes();
        let AstKind::PropertyDefinition(field) = nodes.parent_kind(node.id()) else {
            return;
        };
        let AstKind::Class(class) = nodes.parent_kind(nodes.parent_id(nodes.parent_id(node.id())))
        else {
            return;
        };
        let class_name = class.id.as_ref().map_or("", |id| id.name.as_str());
        if self.is_response(file, class_name) {
            let key = field.key.span();
            cx.report(Span::new(decorator.span.start, key.end))
                .arg("name", one_line(key.source_text(file.text)));
        }
    }
}

impl Check {
    fn is_response(&self, file: &File<'_>, class_name: &str) -> bool {
        let has = |list: &[String], words: &[&str]| {
            words
                .iter()
                .any(|word| list.iter().any(|item| item.eq_ignore_ascii_case(word)))
        };
        let class_words = words(class_name);
        if has(&self.responses, &class_words) {
            return true;
        }
        if has(&self.inputs, &class_words) {
            return false;
        }
        file_name_has_word(file, &self.responses)
    }
}

/// `{ required: false, .. }`.
fn is_not_required(argument: &Argument<'_>) -> bool {
    let Argument::ObjectExpression(options) = argument else {
        return false;
    };
    options.properties.iter().any(|property| {
        matches!(property, ObjectPropertyKind::ObjectProperty(property)
            if property.key.is_specific_static_name("required")
                && matches!(&property.value, Expression::BooleanLiteral(value) if !value.value))
    })
}
