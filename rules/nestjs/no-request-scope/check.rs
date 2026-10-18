use kit::oxc::ast::ast::Expression;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[
        AstType::StaticMemberExpression,
        AstType::ComputedMemberExpression,
    ];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let Some(member) = node.kind().as_member_expression_kind() else {
            return;
        };
        if member
            .static_property_name()
            .is_none_or(|name| name != "REQUEST")
        {
            return;
        }
        let Expression::Identifier(scope) = member.object().get_inner_expression() else {
            return;
        };
        let file = cx.file();
        if matches!(imported_name(file, scope),
            Some((module, "Scope")) if package_name(module) == "@nestjs/common")
        {
            cx.report(place(file, node));
        }
    }
}

/// The text that tells this provider from another of the file: from `Scope.REQUEST` to the name
/// of the class whose decorator holds it (`@Injectable({ scope: Scope.REQUEST })`), the provider
/// object around it (`{ provide: REPORTS, useClass: .., scope: Scope.REQUEST }`), or else
/// `Scope.REQUEST` alone.
fn place(file: &File<'_>, member: &AstNode<'_>) -> Span {
    let nodes = file.semantic.nodes();
    let span = member.span();
    let property = nodes.parent_node(member.id());
    let AstKind::ObjectProperty(entry) = property.kind() else {
        return span;
    };
    if entry.value.span() != span {
        return span;
    }
    let object = nodes.parent_node(property.id());
    let mut owner = nodes.ancestor_kinds(object.id());
    match (owner.next(), owner.next(), owner.next()) {
        (
            Some(AstKind::CallExpression(_)),
            Some(AstKind::Decorator(_)),
            Some(AstKind::Class(class)),
        ) => Span::new(
            span.start,
            class
                .id
                .as_ref()
                .map_or(class.body.span.start, |id| id.span.end),
        ),
        _ => object.span(),
    }
}
