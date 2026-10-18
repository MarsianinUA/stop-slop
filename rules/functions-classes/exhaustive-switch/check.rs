use kit::oxc::ast::ast::{Expression, IdentifierReference};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    keys: Vec<String>,
}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::SwitchStatement];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let AstKind::SwitchStatement(switch) = node.kind() else {
            return;
        };
        // Any `default` is the author's choice: without types the rule cannot tell whether the
        // value is wider than the listed members (`switch (raw) { .. default: return null }`).
        if switch.cases.iter().any(|case| case.is_default_case()) {
            return;
        }
        let tests: Vec<&Expression<'_>> = switch
            .cases
            .iter()
            .filter_map(|case| case.test.as_ref())
            .collect();
        let Some(first) = tests.first() else {
            return;
        };
        if !self.is_over_key(&switch.discriminant, &tests) && !is_over_enum(file, &tests) {
            return;
        }
        cx.report(Span::new(switch.span.start, first.span().end))
            .arg(
                "subject",
                one_line(switch.discriminant.span().source_text(file.text)),
            );
    }
}

impl Check {
    /// `switch (shape.kind)` (or `shape?.kind`) with a key from `keys` and only string cases.
    fn is_over_key(&self, discriminant: &Expression<'_>, tests: &[&Expression<'_>]) -> bool {
        discriminant
            .get_member_expr()
            .and_then(|member| member.static_property_name())
            .is_some_and(|key| self.keys.iter().any(|k| k == key))
            && tests.iter().all(|test| match test.get_inner_expression() {
                Expression::StringLiteral(_) => true,
                Expression::TemplateLiteral(template) => template.expressions.is_empty(),
                _ => false,
            })
    }
}

/// Every case is `E.M` (or `E['M']`) with one `E`: a closed set of members.
fn is_over_enum(file: &File<'_>, tests: &[&Expression<'_>]) -> bool {
    let Some(object) = tests.first().and_then(|test| enum_object(test)) else {
        return false;
    };
    is_closed_set(file, object)
        && tests
            .iter()
            .all(|test| enum_object(test).is_some_and(|other| other.name == object.name))
}

/// `E.M` or `E['M']`: the `E`.
fn enum_object<'a>(test: &'a Expression<'a>) -> Option<&'a IdentifierReference<'a>> {
    let member = test.get_inner_expression().as_member_expression()?;
    member.static_property_name()?;
    match member.object() {
        Expression::Identifier(id) => Some(id),
        _ => None,
    }
}

/// An enum of the file, an import, or a `const` object literal (`const Mode = { .. } as const`).
/// A parameter or a `let` holds any object: `case keys.up` does not name a set.
fn is_closed_set(file: &File<'_>, object: &IdentifierReference<'_>) -> bool {
    let scoping = file.semantic.scoping();
    let Some(symbol) = scoping.get_reference(object.reference_id()).symbol_id() else {
        return false;
    };
    let flags = scoping.symbol_flags(symbol);
    if flags.is_enum() || flags.is_import() {
        return true;
    }
    let nodes = file.semantic.nodes();
    let declaration = scoping.symbol_declaration(symbol);
    matches!(nodes.kind(declaration), AstKind::VariableDeclarator(declarator)
    if declarator.id.is_binding_identifier()
        && declarator.init.as_ref().is_some_and(|init| {
            matches!(init.get_inner_expression(), Expression::ObjectExpression(_))
        }))
        && matches!(nodes.parent_kind(declaration), AstKind::VariableDeclaration(list)
            if list.kind.is_const())
}
