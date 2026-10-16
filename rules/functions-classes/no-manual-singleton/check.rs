use kit::oxc::ast::ast::{
    AssignmentOperator, ClassElement, Expression, MethodDefinitionKind, TSType, TSTypeName,
};
use kit::oxc::semantic::NodeId;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::Class];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let AstKind::Class(class) = node.kind() else {
            return;
        };
        if class.declare {
            return;
        }
        let nodes = file.semantic.nodes();
        let id = match (&class.id, nodes.parent_kind(node.id())) {
            (Some(id), _) => Some(id),
            (None, AstKind::VariableDeclarator(declarator)) => {
                declarator.id.get_binding_identifier()
            }
            _ => None,
        };
        let Some(name) = id.map(|id| id.name.as_str()) else {
            return;
        };
        // Judged by shape, not by the words `instance` / `getInstance`: the slot may be `shared`,
        // `current` or `default`.
        let mut slots: Vec<Slot> = class
            .body
            .body
            .iter()
            .filter_map(|element| slot(file, element, name))
            .collect();
        if slots.is_empty() {
            return;
        }
        let body = class.body.node_id.get();
        let end = class.body.span.end;
        // Node ids follow the source in pre-order: the body's nodes come right after it, and the
        // first node past them starts after its `}`.
        for inner in nodes
            .iter()
            .skip(body.index() + 1)
            .take_while(|inner| inner.span().start < end)
        {
            let Some(field) = member_field(file, inner.kind(), name) else {
                continue;
            };
            for slot in slots.iter_mut().filter(|slot| slot.field == field) {
                // `Cache.instance = new Cache()`, `this.#instance ??= new this()`.
                slot.keeps_own |= matches!(
                    nodes.parent_kind(inner.id()),
                    AstKind::AssignmentExpression(assignment)
                        if assignment.left.span() == inner.span()
                            && matches!(
                                assignment.operator,
                                AssignmentOperator::Assign
                                    | AssignmentOperator::LogicalNullish
                                    | AssignmentOperator::LogicalOr
                            )
                            && is_new_of(&assignment.right, name)
                );
                if slot.method.is_none() {
                    slot.method = returning_method(file, inner.id(), body);
                }
            }
        }
        // Several instances kept in static fields (`Color.RED`, `Color.BLUE`) are an enum-like
        // set of values, not one instance.
        if slots.iter().filter(|slot| slot.keeps_own).count() > 1 {
            return;
        }
        for slot in slots {
            if slot.keeps_own
                && let Some(method) = slot.method
            {
                cx.report(slot.span)
                    .arg("class", name)
                    .arg("field", slot.field)
                    .arg("method", method);
            }
        }
    }
}

struct Slot<'a> {
    /// `instance`, `#instance`.
    field: &'a str,
    span: Span,
    /// Typed as the class, or set to `new C()`.
    keeps_own: bool,
    /// The first static method or getter that returns the slot.
    method: Option<&'a str>,
}

fn slot<'a>(file: &File<'a>, element: &ClassElement<'a>, class: &str) -> Option<Slot<'a>> {
    let (key, annotation, value) = match element {
        ClassElement::PropertyDefinition(property) if property.r#static && !property.declare => {
            (&property.key, &property.type_annotation, &property.value)
        }
        ClassElement::AccessorProperty(accessor) if accessor.r#static => {
            (&accessor.key, &accessor.type_annotation, &accessor.value)
        }
        _ => return None,
    };
    if !key.is_identifier() {
        return None;
    }
    Some(Slot {
        field: key.span().source_text(file.text),
        span: element.span(),
        keeps_own: annotation
            .as_ref()
            .is_some_and(|annotation| names_class(&annotation.type_annotation, class))
            || value.as_ref().is_some_and(|value| is_new_of(value, class)),
        method: None,
    })
}

/// `C`, `C | null`.
fn names_class(ty: &TSType<'_>, class: &str) -> bool {
    match ty {
        TSType::TSTypeReference(reference) => {
            matches!(&reference.type_name, TSTypeName::IdentifierReference(id) if id.name == class)
        }
        TSType::TSUnionType(union) => union.types.iter().any(|ty| names_class(ty, class)),
        _ => false,
    }
}

/// `new C(..)`, `new this(..)`.
fn is_new_of(expression: &Expression<'_>, class: &str) -> bool {
    matches!(expression.get_inner_expression(), Expression::NewExpression(new)
        if is_class(&new.callee, class))
}

/// `this` in a static method is the class.
fn is_class(expression: &Expression<'_>, class: &str) -> bool {
    match expression.get_inner_expression() {
        Expression::ThisExpression(_) => true,
        Expression::Identifier(id) => id.name == class,
        _ => false,
    }
}

/// `C.slot`, `this.slot`, `C.#slot`, `this.#slot`.
fn member_field<'a>(file: &File<'a>, kind: AstKind<'a>, class: &str) -> Option<&'a str> {
    let (object, field) = match kind {
        AstKind::StaticMemberExpression(member) => (&member.object, member.property.span),
        AstKind::PrivateFieldExpression(member) => (&member.object, member.field.span),
        _ => return None,
    };
    is_class(object, class).then(|| field.source_text(file.text))
}

/// The key of the class's static method or getter whose own `return` holds the node: a `return`
/// in a nested function returns from that function.
fn returning_method<'a>(file: &File<'a>, id: NodeId, body: NodeId) -> Option<&'a str> {
    let nodes = file.semantic.nodes();
    let mut ancestors = nodes.ancestor_ids(id);
    let nearest = ancestors.find(|&ancestor| {
        let kind = nodes.kind(ancestor);
        matches!(kind, AstKind::ReturnStatement(_)) || kind.is_function_like()
    })?;
    if !matches!(nodes.kind(nearest), AstKind::ReturnStatement(_)) {
        return None;
    }
    let function = ancestors.find(|&ancestor| nodes.kind(ancestor).is_function_like())?;
    let method = nodes.parent_id(function);
    match nodes.kind(method) {
        AstKind::MethodDefinition(method_definition)
            if method_definition.r#static
                && matches!(
                    method_definition.kind,
                    MethodDefinitionKind::Method | MethodDefinitionKind::Get
                )
                && nodes.parent_id(method) == body =>
        {
            Some(method_definition.key.span().source_text(file.text))
        }
        _ => None,
    }
}
