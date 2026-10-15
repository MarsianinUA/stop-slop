use kit::Facts;
use kit::oxc::ast::ast::{TSIntersectionType, TSType, TSTypeName};
use kit::prelude::*;

const UTILITIES: &[&str] = &[
    "Omit",
    "Pick",
    "Partial",
    "Required",
    "Readonly",
    "NonNullable",
    "Exclude",
    "Extract",
];

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const USES: &'static [Fact] = &[Fact::Aliases, Fact::Packages];
    const NODES: &'static [AstType] = &[
        AstType::TSTypeReference,
        AstType::TSInterfaceHeritage,
        AstType::TSClassImplements,
        AstType::TSIntersectionType,
    ];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let facts = cx.facts();
        let found = match node.kind() {
            // `type X = Own & { .. }` names a base type extended, the fix this rule asks for.
            AstKind::TSIntersectionType(intersection)
                if !matches!(
                    file.semantic.nodes().parent_kind(node.id()),
                    AstKind::TSTypeAliasDeclaration(_)
                ) =>
            {
                extended(file, facts, intersection).map(|own| ("&", own))
            }
            kind => reshaped(file, facts, kind),
        };
        let Some((utility, own)) = found else {
            return;
        };
        cx.report(node.span())
            .arg("utility", utility)
            .arg("type", own.span().source_text(file.text));
    }
}

/// The utility's name and its first argument, when a lib utility reshapes a type of this
/// project. An unresolved name is the lib one: a file's own `type Partial<T>` is not.
fn reshaped<'a>(
    file: &File<'_>,
    facts: &dyn Facts,
    kind: AstKind<'a>,
) -> Option<(&'a str, &'a TSType<'a>)> {
    let (name, arguments) = match kind {
        AstKind::TSTypeReference(r) => (&r.type_name, r.type_arguments.as_deref()),
        AstKind::TSInterfaceHeritage(h) => (&h.type_name, h.type_arguments.as_deref()),
        AstKind::TSClassImplements(i) => (&i.expression, i.type_arguments.as_deref()),
        _ => return None,
    };
    let TSTypeName::IdentifierReference(id) = name else {
        return None;
    };
    if !UTILITIES.contains(&id.name.as_str()) || !file.semantic.is_reference_to_global_variable(id)
    {
        return None;
    }
    let first = arguments?.params.first()?;
    is_own_type(file, facts, first).then_some((id.name.as_str(), first))
}

/// The own member of `Own & { .. }`.
fn extended<'a>(
    file: &File<'_>,
    facts: &dyn Facts,
    intersection: &'a TSIntersectionType<'a>,
) -> Option<&'a TSType<'a>> {
    if !intersection
        .types
        .iter()
        .any(|t| matches!(t, TSType::TSTypeLiteral(_)))
    {
        return None;
    }
    intersection
        .types
        .iter()
        .find(|t| is_own_type(file, facts, t))
}
