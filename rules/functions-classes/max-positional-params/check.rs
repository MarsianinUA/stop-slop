use kit::oxc::ast::ast::{
    BindingPattern, FormalParameters, MethodDefinitionKind, PropertyKey, TSAccessibility,
};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    max: u32,
}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::Function, AstType::ArrowFunctionExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let params = match node.kind() {
            // Without a body it is a signature: an overload, an `abstract` method, a `declare`.
            AstKind::Function(function) if function.body.is_some() => &function.params,
            AstKind::ArrowFunctionExpression(arrow) => &arrow.params,
            _ => return,
        };
        // A decorated parameter is filled by the framework; a destructured object is the options.
        let count = params
            .items
            .iter()
            .filter(|parameter| {
                parameter.decorators.is_empty()
                    && !matches!(parameter.pattern, BindingPattern::ObjectPattern(_))
            })
            .count()
            + usize::from(params.rest.is_some());
        if count <= self.max as usize
            || is_ambient(file, node.id())
            || is_signature_given(file, node)
            || is_injected(file, node, params)
        {
            return;
        }
        cx.report(params.span)
            .arg("count", count)
            .arg("max", self.max);
    }
}

/// A constructor that DI fills: the class has a decorator (`@Injectable()`), or a parameter is a
/// property (`private readonly store: Store`) or has a decorator (`@Inject(TOKEN)`).
fn is_injected(file: &File<'_>, function: &AstNode<'_>, params: &FormalParameters<'_>) -> bool {
    let nodes = file.semantic.nodes();
    let method = nodes.parent_node(function.id());
    matches!(method.kind(), AstKind::MethodDefinition(definition)
        if definition.kind == MethodDefinitionKind::Constructor)
        && (params
            .items
            .iter()
            .any(|parameter| parameter.has_modifier() || !parameter.decorators.is_empty())
            // method -> `ClassBody` -> `Class`.
            || matches!(
                nodes.parent_kind(nodes.parent_id(method.id())),
                AstKind::Class(class) if !class.decorators.is_empty()
            ))
}

/// Someone else sets the signature: the caller of a callback, the property of a setter, the base
/// class of an `override`, the interface of a class that `implements` one (for its public
/// members), the type of the variable or field that holds the function
/// (`const handler: RequestHandler = (req, res, next) => ..`), the type it `satisfies` or is
/// asserted to, the return type of the function that returns it
/// (`function audit(): RequestHandler { return (req, res, next) => .. }`).
fn is_signature_given(file: &File<'_>, function: &AstNode<'_>) -> bool {
    if is_callback(file, function) {
        return true;
    }
    let nodes = file.semantic.nodes();
    let parent = nodes.parent_node(function.id());
    match parent.kind() {
        AstKind::VariableDeclarator(declarator) => declarator.type_annotation.is_some(),
        AstKind::TSSatisfiesExpression(_)
        | AstKind::TSAsExpression(_)
        | AstKind::TSTypeAssertion(_) => true,
        // The body of an arrow: `(prefix): RequestHandler => (req, res, next) => ..`.
        AstKind::ArrowFunctionExpression(arrow) => arrow.return_type.is_some(),
        AstKind::ReturnStatement(_) => nodes
            .ancestor_kinds(parent.id())
            .find_map(|kind| match kind {
                AstKind::Function(outer) => Some(outer.return_type.is_some()),
                AstKind::ArrowFunctionExpression(outer) => Some(outer.return_type.is_some()),
                _ => None,
            })
            .unwrap_or(false),
        AstKind::PropertyDefinition(field) => {
            field.type_annotation.is_some()
                || !field.r#static
                    && is_public(field.accessibility, &field.key)
                    && implements(file, parent)
        }
        AstKind::MethodDefinition(method) => {
            method.kind == MethodDefinitionKind::Set
                || method.r#override
                || method.kind == MethodDefinitionKind::Method
                    && !method.r#static
                    && is_public(method.accessibility, &method.key)
                    && implements(file, parent)
        }
        _ => false,
    }
}

fn is_public(accessibility: Option<TSAccessibility>, key: &PropertyKey<'_>) -> bool {
    !key.is_private_identifier()
        && !matches!(
            accessibility,
            Some(TSAccessibility::Private | TSAccessibility::Protected)
        )
}

/// member -> `ClassBody` -> `Class` with `implements`.
fn implements(file: &File<'_>, member: &AstNode<'_>) -> bool {
    let nodes = file.semantic.nodes();
    matches!(
        nodes.parent_kind(nodes.parent_id(member.id())),
        AstKind::Class(class) if !class.implements.is_empty()
    )
}
