use kit::oxc::ast::ast::{
    Argument, BindingPattern, ClassElement, Expression, IdentifierReference, MethodDefinitionKind,
    PropertyKey, TSType, TSTypeAnnotation, TSTypeName,
};
use kit::oxc::semantic::NodeId;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::CallExpression(call) = node.kind() else {
            return;
        };
        let Some(member) = call.callee.get_member_expr() else {
            return;
        };
        if member.static_property_name() != Some("get") {
            return;
        }
        let file = cx.file();
        if is_config_service(file, node.id(), member.object().get_inner_expression()) {
            cx.report(call.span)
                .arg("call", one_line(call.span.source_text(file.text)));
        }
    }
}

/// The member `this.config` or `this.#config` reads.
enum Field<'n> {
    Public(&'n str),
    Private(&'n str),
}

/// The value is a `ConfigService` of `@nestjs/config` by its own syntax: a name or a `this` field
/// typed so (a parameter, a variable, a field, a constructor parameter property), a `const`
/// holding the service taken from the container (`app.get(ConfigService)`), or that call itself.
fn is_config_service(file: &File<'_>, at: NodeId, value: &Expression<'_>) -> bool {
    match value {
        Expression::Identifier(id) => {
            let scoping = file.semantic.scoping();
            let Some(symbol) = scoping.get_reference(id.reference_id()).symbol_id() else {
                return false;
            };
            let nodes = file.semantic.nodes();
            let declaration = scoping.symbol_declaration(symbol);
            match nodes.kind(declaration) {
                AstKind::FormalParameter(parameter) => {
                    is_config_type(file, parameter.type_annotation.as_deref())
                }
                AstKind::VariableDeclarator(declarator) => {
                    is_config_type(file, declarator.type_annotation.as_deref())
                        || matches!(nodes.parent_kind(declaration),
                            AstKind::VariableDeclaration(list) if list.kind.is_const())
                            && declarator
                                .init
                                .as_ref()
                                .is_some_and(|init| is_resolved(file, init))
                }
                _ => false,
            }
        }
        Expression::StaticMemberExpression(member)
            if matches!(member.object, Expression::ThisExpression(_)) =>
        {
            is_config_field(file, at, &Field::Public(member.property.name.as_str()))
        }
        Expression::PrivateFieldExpression(member)
            if matches!(member.object, Expression::ThisExpression(_)) =>
        {
            is_config_field(file, at, &Field::Private(member.field.name.as_str()))
        }
        _ => is_resolved(file, value),
    }
}

/// A field of the class whose `this` the call sees, or a parameter property of its constructor,
/// typed `ConfigService`.
fn is_config_field(file: &File<'_>, at: NodeId, field: &Field<'_>) -> bool {
    let Some(class) = this_class(file, at) else {
        return false;
    };
    class.body.body.iter().any(|element| match element {
        ClassElement::PropertyDefinition(property) => {
            !property.r#static
                && has_key(&property.key, field)
                && is_config_type(file, property.type_annotation.as_deref())
        }
        ClassElement::MethodDefinition(method)
            if method.kind == MethodDefinitionKind::Constructor =>
        {
            let Field::Public(name) = field else {
                return false;
            };
            method.value.params.items.iter().any(|parameter| {
                parameter.has_modifier()
                    && matches!(&parameter.pattern,
                        BindingPattern::BindingIdentifier(id) if id.name == *name)
                    && is_config_type(file, parameter.type_annotation.as_deref())
            })
        }
        _ => false,
    })
}

fn has_key(key: &PropertyKey<'_>, field: &Field<'_>) -> bool {
    match (key, field) {
        (PropertyKey::PrivateIdentifier(id), Field::Private(name)) => id.name == *name,
        (_, Field::Public(name)) => key.static_name().is_some_and(|key| key == *name),
        _ => false,
    }
}

/// `ConfigService` or `ConfigService<Env, true>`.
fn is_config_type(file: &File<'_>, annotation: Option<&TSTypeAnnotation<'_>>) -> bool {
    let Some(TSType::TSTypeReference(reference)) =
        annotation.map(|annotation| &annotation.type_annotation)
    else {
        return false;
    };
    matches!(&reference.type_name,
        TSTypeName::IdentifierReference(name) if is_config_class(file, name))
}

fn is_config_class(file: &File<'_>, name: &IdentifierReference<'_>) -> bool {
    matches!(imported_name(file, name),
        Some((module, "ConfigService")) if package_name(module) == "@nestjs/config")
}

/// The service taken from the container: `app.get(ConfigService)`,
/// `await moduleRef.resolve(ConfigService)`, with type arguments and options too.
fn is_resolved(file: &File<'_>, value: &Expression<'_>) -> bool {
    let value = match value.get_inner_expression() {
        Expression::AwaitExpression(wait) => &wait.argument,
        value => value,
    };
    let Some(call) = call_of(value) else {
        return false;
    };
    call.callee
        .get_member_expr()
        .is_some_and(|member| matches!(member.static_property_name(), Some("get" | "resolve")))
        && matches!(call.arguments.first(),
        Some(Argument::Identifier(class)) if is_config_class(file, class))
}
