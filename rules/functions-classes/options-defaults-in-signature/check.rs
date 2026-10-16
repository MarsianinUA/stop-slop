use kit::oxc::ast::ast::{BindingPattern, Expression, FormalParameter, LogicalOperator};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::LogicalExpression, AstType::VariableDeclarator];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        match node.kind() {
            AstKind::LogicalExpression(logical)
                if matches!(
                    logical.operator,
                    LogicalOperator::Coalesce | LogicalOperator::Or
                ) =>
            {
                let Some(member) = logical.left.get_member_expr() else {
                    return;
                };
                let Some(key) = member.static_property_name() else {
                    return;
                };
                let Some((name, parameter)) = parameter(file, member.object()) else {
                    return;
                };
                if is_option_key(file, parameter, key) {
                    cx.report(logical.span).arg("name", name).arg("key", key);
                }
            }
            // A default says the key may be missing, whatever the type.
            AstKind::VariableDeclarator(declarator) => {
                let BindingPattern::ObjectPattern(pattern) = &declarator.id else {
                    return;
                };
                let Some(key) =
                    pattern
                        .properties
                        .iter()
                        .find_map(|property| match property.value {
                            BindingPattern::AssignmentPattern(_) => property.key.static_name(),
                            _ => None,
                        })
                else {
                    return;
                };
                let Some((name, _)) = declarator
                    .init
                    .as_ref()
                    .and_then(|init| parameter(file, without_empty_fallback(init)))
                else {
                    return;
                };
                cx.report(declarator.span).arg("name", name).arg("key", key);
            }
            _ => {}
        }
    }
}

/// The parameter that `expression` names, unless its function is a callback, whose parameters the
/// caller sets, or it is a parameter property, which cannot be destructured in the signature. A
/// destructured parameter declares its names with the same node, so only a parameter that is one
/// name counts.
fn parameter<'a>(
    file: &File<'a>,
    expression: &Expression<'_>,
) -> Option<(&'a str, &'a FormalParameter<'a>)> {
    let Expression::Identifier(id) = expression.get_inner_expression() else {
        return None;
    };
    let scoping = file.semantic.scoping();
    let nodes = file.semantic.nodes();
    let symbol = scoping.get_reference(id.reference_id()).symbol_id()?;
    let declaration = scoping.symbol_declaration(symbol);
    let AstKind::FormalParameter(parameter) = nodes.kind(declaration) else {
        return None;
    };
    let BindingPattern::BindingIdentifier(binding) = &parameter.pattern else {
        return None;
    };
    if parameter.has_modifier() {
        return None;
    }
    // parameter -> `FormalParameters` -> function.
    let function = nodes.parent_node(nodes.parent_id(declaration));
    (!is_callback(file, function)).then_some((binding.name.as_str(), parameter))
}

/// `options ?? {}` or `options || {}` -> `options`.
fn without_empty_fallback<'a>(init: &'a Expression<'a>) -> &'a Expression<'a> {
    match init.get_inner_expression() {
        Expression::LogicalExpression(logical)
            if matches!(
                logical.operator,
                LogicalOperator::Coalesce | LogicalOperator::Or
            ) && matches!(
                logical.right.get_inner_expression(),
                Expression::ObjectExpression(object) if object.properties.is_empty()
            ) =>
        {
            &logical.left
        }
        _ => init,
    }
}
