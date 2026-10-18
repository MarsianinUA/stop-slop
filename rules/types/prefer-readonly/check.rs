use std::borrow::Cow;

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
        // `this[key]`: any field of the class may be written or read there.
        let mut dynamic_writes = Vec::new();
        let mut dynamic_reads = Vec::new();
        for node in nodes.iter() {
            let reached = match node.kind() {
                AstKind::PropertyDefinition(property) => {
                    fields.extend(property_field(file, node, property));
                    continue;
                }
                AstKind::FormalParameter(parameter) => {
                    fields.extend(parameter_field(file, node, parameter));
                    continue;
                }
                _ => match member_use(file, node) {
                    Some(reached) => reached,
                    None => continue,
                },
            };
            let access = match node.kind().as_member_expression_kind() {
                Some(member) if is_write(file, node) => {
                    // The constructor may set a `readonly` field through `this` only, not
                    // through `parent.children = ..`, and never `delete` it.
                    let on_this = matches!(member.object(), Expression::ThisExpression(_));
                    let deleted = matches!(
                        nodes.parent_kind(node.id()),
                        AstKind::UnaryExpression(unary) if unary.operator == UnaryOperator::Delete
                    );
                    Access::Write(
                        (on_this && !deleted)
                            .then(|| enclosing_function(file, node))
                            .flatten(),
                    )
                }
                Some(_) => Access::Read(keeps_primitive(file, node)),
                // `const { mode } = this` reads a field without a member expression.
                None => Access::Read(false),
            };
            match reached {
                MemberUse::Name(name) => uses.push(Use {
                    name,
                    span: node.span(),
                    access,
                }),
                MemberUse::Any => match access {
                    Access::Write(_) => dynamic_writes.push(node.span()),
                    Access::Read(_) => dynamic_reads.push(node.span()),
                },
            }
        }

        for field in fields {
            let inside = |span: Span| field.class.contains_inclusive(span);
            if dynamic_writes.iter().any(|&span| inside(span)) {
                continue;
            }
            let field_uses: Vec<&Use> = uses
                .iter()
                .filter(|u| u.name == field.name.as_str() && inside(u.span))
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
            // another value, a comparison, a generic call or an alias would stop compiling.
            let noticed = field_uses
                .iter()
                .any(|u| !matches!(u.access, Access::Read(true)))
                || dynamic_reads.iter().any(|&span| inside(span));
            if !(field.narrows && noticed) {
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

struct Use<'a> {
    name: Cow<'a, str>,
    span: Span,
    access: Access,
}

enum Access {
    /// With the nearest enclosing function, static block or field initializer of a write
    /// through `this`; `None` for a write through another object and for `delete`.
    Write(Option<NodeId>),
    /// True where a literal type behaves like its primitive.
    Read(bool),
}

fn property_field(
    file: &File<'_>,
    node: &AstNode<'_>,
    property: &PropertyDefinition<'_>,
) -> Option<Field> {
    let private = property.accessibility == Some(TSAccessibility::Private)
        || property.key.is_private_identifier();
    // A framework sets a decorated or `!` field after the object is built.
    if !private
        || property.readonly
        || property.declare
        || property.computed
        || property.definite
        || !property.decorators.is_empty()
        || property.r#type != PropertyDefinitionType::PropertyDefinition
    {
        return None;
    }
    let nodes = file.semantic.nodes();
    let AstKind::Class(class) = nodes.parent_kind(nodes.parent_id(node.id())) else {
        return None;
    };
    // The code of a `declare class` is elsewhere.
    if class.declare {
        return None;
    }
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

/// The member is assigned, updated, deleted, destructured into or a `for` target, also
/// through `!`, `as`, `<T>` and `satisfies`.
fn is_write(file: &File<'_>, node: &AstNode<'_>) -> bool {
    let target = outer_assertion(file, node);
    let span = target.span();
    match file.semantic.nodes().parent_kind(target.id()) {
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
