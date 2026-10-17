use kit::oxc::ast::ast::{Expression, ObjectPropertyKind, UnaryOperator};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::TSEnumDeclaration, AstType::VariableDeclarator];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        match node.kind() {
            AstKind::TSEnumDeclaration(declaration) => {
                let name = declaration.id.name.as_str();
                // `declare enum` describes codes someone else assigned.
                if !is_code_name(name) || is_ambient(file, node.id()) {
                    return;
                }
                for member in &declaration.body.members {
                    if !member.initializer.as_ref().is_some_and(is_number) {
                        cx.report(member.span)
                            .arg("member", member.id.static_name())
                            .arg("name", name);
                    }
                }
            }
            // `const OrderErrorCode = { NotFound: 10702 } as const`.
            AstKind::VariableDeclarator(declarator) => {
                let Some(id) = declarator.id.get_binding_identifier() else {
                    return;
                };
                let name = id.name.as_str();
                let Some(Expression::ObjectExpression(object)) = declarator
                    .init
                    .as_ref()
                    .map(Expression::get_inner_expression)
                else {
                    return;
                };
                if !is_code_name(name) {
                    return;
                }
                for property in &object.properties {
                    // A spread brings codes declared elsewhere.
                    let ObjectPropertyKind::ObjectProperty(property) = property else {
                        continue;
                    };
                    if !is_number(&property.value) {
                        cx.report(property.span)
                            .arg("member", property.key.span().source_text(file.text))
                            .arg("name", name);
                    }
                }
            }
            _ => {}
        }
    }
}

/// `ErrorCode`, `PriceFeedErrorCode`, `ERROR_CODES`.
fn is_code_name(name: &str) -> bool {
    matches!(words(name).as_slice(), [.., error, code]
        if error.eq_ignore_ascii_case("error")
            && (code.eq_ignore_ascii_case("code") || code.eq_ignore_ascii_case("codes")))
}

/// `10702`, `-1`, also under `as const`.
fn is_number(expression: &Expression<'_>) -> bool {
    match expression.get_inner_expression() {
        Expression::NumericLiteral(_) => true,
        Expression::UnaryExpression(unary) => {
            unary.operator == UnaryOperator::UnaryNegation && unary.argument.is_number()
        }
        _ => false,
    }
}
