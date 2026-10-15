use kit::oxc::ast::MemberExpressionKind;
use kit::oxc::ast::ast::{
    Class, ClassElement, Expression, FormalParameter, MethodDefinitionKind, PropertyDefinition,
    PropertyDefinitionType, PropertyKey, TSAccessibility, UnaryOperator,
};
use kit::oxc::semantic::NodeId;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let nodes = file.semantic.nodes();
        let mut fields = Vec::new();
        let mut uses = Vec::new();
        // `this[key] = ..`: any field of the class may be written there.
        let mut dynamic_writes = Vec::new();
        for node in nodes.iter() {
            let member = match node.kind() {
                AstKind::PropertyDefinition(property) => {
                    fields.extend(property_field(file, node, property));
                    continue;
                }
                AstKind::FormalParameter(parameter) => {
                    fields.extend(parameter_field(file, node, parameter));
                    continue;
                }
                kind => match kind.as_member_expression_kind() {
                    Some(member) => member,
                    None => continue,
                },
            };
            let access = if is_write(file, node) {
                // The constructor may set a `readonly` field through `this` only, not through
                // `parent.children = ..`.
                let on_this = matches!(member.object(), Expression::ThisExpression(_));
                Access::Write(on_this.then(|| enclosing_function(file, node)).flatten())
            } else if is_compared(file, node) {
                Access::Compare
            } else {
                continue;
            };
            if let Some(name) = member_name(&member) {
                uses.push(Use {
                    name,
                    span: node.span(),
                    access,
                });
            } else if matches!(access, Access::Write(_))
                && matches!(member.object(), Expression::ThisExpression(_))
            {
                dynamic_writes.push(node.span());
            }
        }

        for field in fields {
            let inside = |span: Span| field.class.contains_inclusive(span);
            if dynamic_writes.iter().any(|&span| inside(span)) {
                continue;
            }
            let field_uses: Vec<&Use> = uses
                .iter()
                .filter(|u| u.name == field.name && inside(u.span))
                .collect();
            let written_elsewhere = field_uses.iter().any(|u| {
                matches!(u.access, Access::Write(function)
                    if function.is_none() || function != field.constructor)
            });
            if written_elsewhere {
                continue;
            }
            let reported = cx.report(field.span).arg("name", &field.name);
            // `private readonly mode = 'fast'` has the type `'fast'`: a constructor that sets
            // another value, or a comparison with one, would stop compiling.
            if !(field.narrows && !field_uses.is_empty()) {
                reported.fix([Edit {
                    span: Span::empty(field.span.start),
                    text: "readonly ".into(),
                }]);
            }
        }
    }
}

struct Field {
    /// `#requests` keeps its `#`.
    name: String,
    class: Span,
    /// The constructor's function: the only place allowed to write the field. `None` for a
    /// static field.
    constructor: Option<NodeId>,
    /// From the name to the end of the annotation; `readonly ` goes at its start, after the
    /// modifiers and decorators.
    span: Span,
    /// With `readonly` the field would get the literal type of its value.
    narrows: bool,
}

struct Use {
    name: String,
    span: Span,
    access: Access,
}

enum Access {
    /// With the nearest enclosing function, static block or field initializer of a write
    /// through `this`; `None` for a write through another object.
    Write(Option<NodeId>),
    Compare,
}

fn property_field(
    file: &File<'_>,
    node: &AstNode<'_>,
    property: &PropertyDefinition<'_>,
) -> Option<Field> {
    let private = property.accessibility == Some(TSAccessibility::Private)
        || property.key.is_private_identifier();
    if !private
        || property.readonly
        || property.declare
        || property.computed
        || property.r#type != PropertyDefinitionType::PropertyDefinition
    {
        return None;
    }
    let nodes = file.semantic.nodes();
    let AstKind::Class(class) = nodes.parent_kind(nodes.parent_id(node.id())) else {
        return None;
    };
    let key = property.key.span();
    let end = property
        .type_annotation
        .as_ref()
        .map_or(key.end, |annotation| annotation.span.end);
    Some(Field {
        name: key_name(&property.key)?,
        class: class.span,
        constructor: if property.r#static {
            None
        } else {
            constructor(class)
        },
        span: Span::new(key.start, end),
        narrows: property.type_annotation.is_none()
            && property.value.as_ref().is_some_and(narrows_to_literal),
    })
}

/// `constructor(private repo: Repo)`: parameters -> function -> constructor -> class body -> class.
fn parameter_field(
    file: &File<'_>,
    node: &AstNode<'_>,
    parameter: &FormalParameter<'_>,
) -> Option<Field> {
    if parameter.accessibility != Some(TSAccessibility::Private) || parameter.readonly {
        return None;
    }
    let nodes = file.semantic.nodes();
    let function = nodes.parent_id(nodes.parent_id(node.id()));
    let method = nodes.parent_id(function);
    let AstKind::MethodDefinition(method_definition) = nodes.kind(method) else {
        return None;
    };
    let AstKind::Class(class) = nodes.parent_kind(nodes.parent_id(method)) else {
        return None;
    };
    if method_definition.kind != MethodDefinitionKind::Constructor {
        return None;
    }
    let name = parameter.pattern.span();
    let end = parameter
        .type_annotation
        .as_ref()
        .map_or(name.end, |annotation| annotation.span.end);
    Some(Field {
        name: parameter.pattern.get_identifier_name()?.to_string(),
        class: class.span,
        constructor: Some(function),
        span: Span::new(name.start, end),
        // A parameter's type is widened, `readonly` or not.
        narrows: false,
    })
}

/// The constructor with a body; overload signatures come before it.
fn constructor(class: &Class<'_>) -> Option<NodeId> {
    class.body.body.iter().find_map(|element| match element {
        ClassElement::MethodDefinition(method)
            if method.kind == MethodDefinitionKind::Constructor && method.value.body.is_some() =>
        {
            Some(method.value.node_id.get())
        }
        _ => None,
    })
}

fn key_name(key: &PropertyKey<'_>) -> Option<String> {
    match key {
        PropertyKey::PrivateIdentifier(id) => Some(format!("#{}", id.name)),
        _ => key.static_name().map(String::from),
    }
}

/// `x` of `.x` and `['x']`, `#x` of `.#x`; `None` for `[key]`.
fn member_name(member: &MemberExpressionKind<'_>) -> Option<String> {
    match member {
        MemberExpressionKind::PrivateField(member) => Some(format!("#{}", member.field.name)),
        _ => member.static_property_name().map(|name| name.to_string()),
    }
}

/// The value may be a literal (`'fast'`, `null`, `Kind.User`, `cond ? 'a' : 'b'`) that a
/// `readonly` field types as itself; an object, a call, `new`, `a + b`... keep their type.
fn narrows_to_literal(value: &Expression<'_>) -> bool {
    !matches!(
        value,
        Expression::ObjectExpression(_)
            | Expression::ArrayExpression(_)
            | Expression::FunctionExpression(_)
            | Expression::ArrowFunctionExpression(_)
            | Expression::ClassExpression(_)
            | Expression::NewExpression(_)
            | Expression::CallExpression(_)
            | Expression::RegExpLiteral(_)
            | Expression::TaggedTemplateExpression(_)
            | Expression::BinaryExpression(_)
            | Expression::TSAsExpression(_)
            | Expression::TSTypeAssertion(_)
    ) && !matches!(value, Expression::TemplateLiteral(template) if !template.expressions.is_empty())
}

/// The member is assigned, updated, deleted, destructured into or a `for` target, also
/// through `!`, `as`, `<T>` and `satisfies`.
fn is_write(file: &File<'_>, node: &AstNode<'_>) -> bool {
    let nodes = file.semantic.nodes();
    let mut target = node;
    let mut parent = nodes.parent_node(target.id());
    while matches!(
        parent.kind(),
        AstKind::TSNonNullExpression(_)
            | AstKind::TSAsExpression(_)
            | AstKind::TSTypeAssertion(_)
            | AstKind::TSSatisfiesExpression(_)
    ) {
        target = parent;
        parent = nodes.parent_node(target.id());
    }
    let span = target.span();
    match parent.kind() {
        AstKind::AssignmentExpression(assignment) => assignment.left.span() == span,
        AstKind::UpdateExpression(_) | AstKind::ArrayAssignmentTarget(_) => true,
        AstKind::UnaryExpression(unary) => unary.operator == UnaryOperator::Delete,
        AstKind::AssignmentTargetPropertyProperty(property) => property.binding.span() == span,
        AstKind::AssignmentTargetWithDefault(default) => default.binding.span() == span,
        AstKind::AssignmentTargetRest(_) => true,
        AstKind::ForOfStatement(statement) => statement.left.span() == span,
        AstKind::ForInStatement(statement) => statement.left.span() == span,
        _ => false,
    }
}

/// `this.mode === ..`, `switch (this.mode)`, `case this.mode:`.
fn is_compared(file: &File<'_>, node: &AstNode<'_>) -> bool {
    match file.semantic.nodes().parent_kind(node.id()) {
        AstKind::BinaryExpression(binary) => binary.operator.is_equality(),
        AstKind::SwitchStatement(_) | AstKind::SwitchCase(_) => true,
        _ => false,
    }
}

fn enclosing_function(file: &File<'_>, node: &AstNode<'_>) -> Option<NodeId> {
    file.semantic
        .nodes()
        .ancestors(node.id())
        .find(|ancestor| {
            ancestor.kind().is_function_like()
                || matches!(
                    ancestor.kind(),
                    AstKind::StaticBlock(_) | AstKind::PropertyDefinition(_)
                )
        })
        .map(|ancestor| ancestor.id())
}
