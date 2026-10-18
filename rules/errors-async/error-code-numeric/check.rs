use std::borrow::Cow;

use kit::oxc::ast::ast::{
    Expression, ObjectExpression, ObjectPropertyKind, UnaryOperator, VariableDeclarationKind,
};
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
                let is_const = matches!(file.semantic.nodes().parent_kind(node.id()),
                    AstKind::VariableDeclaration(declaration)
                        if declaration.kind == VariableDeclarationKind::Const);
                if !is_const {
                    return;
                }
                let Some(id) = declarator.id.get_binding_identifier() else {
                    return;
                };
                let name = id.name.as_str();
                if !is_code_name(name) {
                    return;
                }
                if let Some(object) = declarator
                    .init
                    .as_ref()
                    .and_then(|init| code_object(file, init))
                {
                    report_codes(cx, object, name);
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

/// An object of codes, also under `as const` / `satisfies` and in global `Object.freeze(..)`.
fn code_object<'a>(file: &File<'_>, value: &'a Expression<'a>) -> Option<&'a ObjectExpression<'a>> {
    if let Expression::ObjectExpression(object) = value.get_inner_expression() {
        return Some(object);
    }
    let call = call_of(value)?;
    if global_member(file, &call.callee) != Some(("Object", "freeze")) {
        return None;
    }
    code_object(file, call.arguments.first()?.as_expression()?)
}

fn report_codes<'a>(cx: &mut Cx<'_, 'a>, object: &'a ObjectExpression<'a>, name: &str) {
    let file = cx.file();
    for property in &object.properties {
        // A spread brings codes declared elsewhere.
        let ObjectPropertyKind::ObjectProperty(property) = property else {
            continue;
        };
        // A group of codes: `{ order: { NotFound: 10701 } }`.
        if let Some(group) = code_object(file, &property.value) {
            report_codes(cx, group, name);
        } else if !is_number(&property.value) {
            let member = property
                .key
                .static_name()
                .unwrap_or_else(|| Cow::Borrowed(property.key.span().source_text(file.text)));
            cx.report(property.span)
                .arg("member", member)
                .arg("name", name);
        }
    }
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
