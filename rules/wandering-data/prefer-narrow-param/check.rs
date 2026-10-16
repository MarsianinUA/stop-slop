use std::borrow::Cow;

use kit::oxc::ast::ast::{BindingPattern, TSType};
use kit::oxc::semantic::{NodeId, SymbolId};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::Function, AstType::ArrowFunctionExpression];
    const USES: &'static [Fact] = &[Fact::Packages, Fact::Aliases];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let (params, return_type) = match node.kind() {
            AstKind::Function(function) if function.body.is_some() => {
                (&function.params, &function.return_type)
            }
            AstKind::ArrowFunctionExpression(arrow) => (&arrow.params, &arrow.return_type),
            _ => return,
        };
        // A type guard's parameter is what it checks: `isPaid(order): order is PaidOrder`.
        let is_guard = return_type.as_ref().is_some_and(|annotation| {
            matches!(annotation.type_annotation, TSType::TSTypePredicate(_))
        });
        if params.items.is_empty()
            || is_guard
            || is_ambient(file, node.id())
            || is_signature_given(file, node)
            || is_di_constructor(file, node)
        {
            return;
        }
        for parameter in &params.items {
            // A parameter property is a field; a decorated parameter is filled by the framework.
            if parameter.has_modifier() || !parameter.decorators.is_empty() {
                continue;
            }
            let BindingPattern::BindingIdentifier(binding) = &parameter.pattern else {
                continue;
            };
            let Some(annotation) = &parameter.type_annotation else {
                continue;
            };
            let ty = unwrapped_type(file, &annotation.type_annotation);
            if !matches!(ty, TSType::TSTypeLiteral(_)) && !is_own_type(file, cx.facts(), ty) {
                continue;
            }
            let Some(field) = binding
                .symbol_id
                .get()
                .and_then(|symbol| only_field(file, symbol))
            else {
                continue;
            };
            // Narrowing an options object to one value would make it a positional flag.
            if is_option_key(file, parameter, &field) {
                continue;
            }
            cx.report(parameter.span)
                .arg("name", binding.name.as_str())
                .arg("field", field);
        }
    }
}

/// The one field that every use of the parameter reads. `None` when it has no use, when a use
/// writes to it, calls its method, or does anything but read a field, or when uses read
/// different fields.
fn only_field<'a>(file: &File<'a>, symbol: SymbolId) -> Option<Cow<'a, str>> {
    let mut field = None;
    for reference in file.semantic.scoping().get_resolved_references(symbol) {
        if member_write(file, reference.node_id()).is_some() {
            return None;
        }
        for name in fields_read(file, reference.node_id())? {
            if *field.get_or_insert_with(|| name.clone()) != name {
                return None;
            }
        }
    }
    field
}

/// The fields a reference reads: `p.x`, `p?.x`, `p['x']` that is not called, or the keys of
/// `const { x } = p` without `...rest`. `None` for any other use.
fn fields_read<'a>(file: &File<'a>, reference: NodeId) -> Option<Vec<Cow<'a, str>>> {
    let nodes = file.semantic.nodes();
    let value = outer_assertion(file, nodes.get_node(reference));
    let parent = nodes.parent_node(value.id());
    if let Some(member) = parent.kind().as_member_expression_kind() {
        if member.object().span() != value.span() {
            return None;
        }
        let name = member.static_property_name()?.as_str();
        let member = outer_assertion(file, parent);
        let is_method = matches!(nodes.parent_kind(member.id()),
            AstKind::CallExpression(call) if call.callee.span() == member.span());
        return (!is_method).then(|| vec![Cow::Borrowed(name)]);
    }
    let AstKind::VariableDeclarator(declarator) = parent.kind() else {
        return None;
    };
    let BindingPattern::ObjectPattern(pattern) = &declarator.id else {
        return None;
    };
    if pattern.rest.is_some()
        || declarator
            .init
            .as_ref()
            .is_none_or(|init| init.span() != value.span())
    {
        return None;
    }
    pattern
        .properties
        .iter()
        .map(|property| property.key.static_name())
        .collect()
}
