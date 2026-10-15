use kit::oxc::ast::ast::{
    ClassBody, ClassElement, Expression, FormalParameters, Function, MethodDefinition,
    MethodDefinitionKind, PropertyDefinition, PropertyKey, TSAccessibility, TSTypeAnnotation,
    UnaryOperator,
};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    // A method or a field value is checked at its `MethodDefinition` or `PropertyDefinition`;
    // `is_exported_node` is false for the function itself, so it is not reported twice.
    const NODES: &'static [AstType] = &[
        AstType::Function,
        AstType::ArrowFunctionExpression,
        AstType::MethodDefinition,
        AstType::PropertyDefinition,
    ];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let signature = match node.kind() {
            AstKind::Function(function) if !is_overload_body(file, function) => function_signature(
                file,
                node,
                function.id.as_ref().map(|id| id.span),
                &function.params,
                function.return_type.as_deref(),
            ),
            AstKind::ArrowFunctionExpression(arrow) => function_signature(
                file,
                node,
                None,
                &arrow.params,
                arrow.return_type.as_deref(),
            ),
            AstKind::MethodDefinition(method) => method_signature(file, node, method),
            AstKind::PropertyDefinition(field) => field_signature(file, node, field),
            _ => None,
        };
        let Some(signature) = signature else {
            return;
        };
        let Some(missing) = signature.missing() else {
            return;
        };
        let start = signature.name.map_or(node.span().start, |name| name.start);
        let name = signature
            .name
            .map_or("default", |name| name.source_text(file.text));
        cx.report(Span::new(start, signature.params.span.end))
            .arg("name", name)
            .arg("missing", missing);
    }
}

struct Signature<'a> {
    /// None for an unnamed default export: the finding then starts at the function.
    name: Option<Span>,
    params: &'a FormalParameters<'a>,
    return_type: Option<&'a TSTypeAnnotation<'a>>,
    /// False for a constructor and a setter: TS rejects a return type there.
    needs_return: bool,
}

impl Signature<'_> {
    fn missing(&self) -> Option<&'static str> {
        let untyped_parameter = self.params.items.iter().any(|parameter| {
            parameter.type_annotation.is_none()
                && !parameter
                    .initializer
                    .as_deref()
                    .is_some_and(is_typed_default)
        }) || self
            .params
            .rest
            .as_ref()
            .is_some_and(|rest| rest.type_annotation.is_none());
        let untyped_return = self.needs_return && self.return_type.is_none();
        match (untyped_parameter, untyped_return) {
            (true, true) => Some("parameter and return types"),
            (true, false) => Some("parameter types"),
            (false, true) => Some("return type"),
            (false, false) => None,
        }
    }
}

/// A function or arrow that leaves the file. The variable it initializes names it, even when
/// the function has its own name; the variable's annotation types it, so it is skipped then.
fn function_signature<'a>(
    file: &File<'a>,
    node: &AstNode<'a>,
    own_name: Option<Span>,
    params: &'a FormalParameters<'a>,
    return_type: Option<&'a TSTypeAnnotation<'a>>,
) -> Option<Signature<'a>> {
    let name = match file.semantic.nodes().parent_kind(node.id()) {
        AstKind::VariableDeclarator(declarator) if declarator.type_annotation.is_some() => {
            return None;
        }
        AstKind::VariableDeclarator(declarator) => Some(declarator.id.span()),
        _ => own_name,
    };
    if !is_exported_node(file, node) {
        return None;
    }
    Some(Signature {
        name,
        params,
        return_type,
        needs_return: true,
    })
}

fn method_signature<'a>(
    file: &File<'a>,
    node: &AstNode<'a>,
    method: &'a MethodDefinition<'a>,
) -> Option<Signature<'a>> {
    let AstKind::ClassBody(body) = file.semantic.nodes().parent_kind(node.id()) else {
        return None;
    };
    if !is_public(method.accessibility, &method.key)
        || !in_exported_class(file, node)
        || is_overload_body_method(body, method)
        || typed_by_getter(body, method)
    {
        return None;
    }
    Some(Signature {
        name: Some(method.key.span()),
        params: &method.value.params,
        return_type: method.value.return_type.as_deref(),
        needs_return: matches!(
            method.kind,
            MethodDefinitionKind::Method | MethodDefinitionKind::Get
        ),
    })
}

/// `handle = (event: string) => ..` without an annotation on the field.
fn field_signature<'a>(
    file: &File<'a>,
    node: &AstNode<'a>,
    field: &'a PropertyDefinition<'a>,
) -> Option<Signature<'a>> {
    if !is_public(field.accessibility, &field.key) || field.type_annotation.is_some() {
        return None;
    }
    let (params, return_type) = match field.value.as_ref()? {
        Expression::ArrowFunctionExpression(arrow) => {
            (&*arrow.params, arrow.return_type.as_deref())
        }
        Expression::FunctionExpression(function) => {
            (&*function.params, function.return_type.as_deref())
        }
        _ => return None,
    };
    if !in_exported_class(file, node) {
        return None;
    }
    Some(Signature {
        name: Some(field.key.span()),
        params,
        return_type,
        needs_return: true,
    })
}

fn is_public(accessibility: Option<TSAccessibility>, key: &PropertyKey<'_>) -> bool {
    !matches!(
        accessibility,
        Some(TSAccessibility::Private | TSAccessibility::Protected)
    ) && !key.is_private_identifier()
}

/// The class of a class member: member -> `ClassBody` -> `Class`.
fn in_exported_class(file: &File<'_>, member: &AstNode<'_>) -> bool {
    let nodes = file.semantic.nodes();
    is_exported_node(file, nodes.parent_node(nodes.parent_id(member.id())))
}

/// A default that shows the parameter's type in the signature: `limit = 10`, `since = new Date()`.
/// `nowMs = Date.now()` or `options = {}` does not.
fn is_typed_default(value: &Expression<'_>) -> bool {
    match value {
        Expression::StringLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::TemplateLiteral(_)
        | Expression::NewExpression(_) => true,
        Expression::UnaryExpression(unary) => {
            unary.operator == UnaryOperator::UnaryNegation
                && matches!(
                    unary.argument,
                    Expression::NumericLiteral(_) | Expression::BigIntLiteral(_)
                )
        }
        _ => false,
    }
}

/// The body after overload signatures: `function parse(input: string): number;` comes first,
/// and the signatures are the boundary.
fn is_overload_body(file: &File<'_>, function: &Function<'_>) -> bool {
    function.body.is_some()
        && function
            .id
            .as_ref()
            .and_then(|id| id.symbol_id.get())
            .is_some_and(|symbol| {
                let first = file.semantic.scoping().symbol_declaration(symbol);
                matches!(
                    file.semantic.nodes().kind(first),
                    AstKind::Function(first) if first.body.is_none()
                )
            })
}

/// A method body next to a bodiless method of the same name and `static` in the same class.
fn is_overload_body_method(body: &ClassBody<'_>, method: &MethodDefinition<'_>) -> bool {
    method.value.body.is_some() && has_twin(body, method, |other| other.value.body.is_none())
}

/// A setter next to a getter of the same name and `static` with a return type: TS takes the
/// setter's parameter type from the getter.
fn typed_by_getter(body: &ClassBody<'_>, setter: &MethodDefinition<'_>) -> bool {
    setter.kind == MethodDefinitionKind::Set
        && has_twin(body, setter, |getter| {
            getter.kind == MethodDefinitionKind::Get && getter.value.return_type.is_some()
        })
}

fn has_twin(
    body: &ClassBody<'_>,
    method: &MethodDefinition<'_>,
    test: impl Fn(&MethodDefinition<'_>) -> bool,
) -> bool {
    let Some(name) = method.key.static_name() else {
        return false;
    };
    body.body.iter().any(|element| {
        matches!(
            element,
            ClassElement::MethodDefinition(other)
                if other.r#static == method.r#static
                    && other.key.static_name().as_ref() == Some(&name)
                    && test(other)
        )
    })
}
