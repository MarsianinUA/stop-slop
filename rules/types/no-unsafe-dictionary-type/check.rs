use kit::oxc::ast::ast::{TSType, TSTypeName, TSTypeParameterInstantiation};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[
        AstType::TSTypeReference,
        AstType::TSIndexSignature,
        AstType::TSMappedType,
        AstType::TSInterfaceHeritage,
        AstType::TSClassImplements,
    ];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let Some(value) = unsafe_dictionary(file, node.kind()) else {
            return;
        };
        let dictionary = node.span();
        let mut span = None;
        for ancestor in file.semantic.nodes().ancestors(node.id()) {
            let kind = ancestor.kind();
            match kind {
                AstKind::TSTypeParameter(parameter)
                    if parameter.constraint.as_ref().is_some_and(|constraint| {
                        constraint.span().contains_inclusive(dictionary)
                    }) =>
                {
                    return;
                }
                // A type test, not a contract: `T extends Record<string, unknown> ? A : B`.
                AstKind::TSConditionalType(conditional)
                    if conditional
                        .extends_type
                        .span()
                        .contains_inclusive(dictionary) =>
                {
                    return;
                }
                // Untrusted data comes in as a loose dictionary and gets narrowed here.
                // A parameter with `private`, `readonly`... declares a field instead.
                AstKind::FormalParameter(parameter) if !parameter.has_modifier() => return,
                AstKind::TSTypePredicate(_)
                | AstKind::TSAsExpression(_)
                | AstKind::TSTypeAssertion(_) => return,
                // The value keeps its own type.
                AstKind::TSSatisfiesExpression(_) => return,
                // The outer dictionary reports.
                _ if unsafe_dictionary(file, kind).is_some() => return,
                _ => {}
            }
            span = span.or_else(|| owner_span(kind, dictionary));
            if !is_type_part(kind) {
                break;
            }
        }
        cx.report(span.unwrap_or(dictionary)).arg("value", value);
    }
}

/// The unsafe value of a dictionary type: lib `Record<K, V>` (also in `extends` and
/// `implements`), `{ [key: K]: V }` or `{ [P in K]: V }`. Any key counts.
fn unsafe_dictionary(file: &File<'_>, kind: AstKind<'_>) -> Option<&'static str> {
    let value = match kind {
        AstKind::TSTypeReference(reference) => record_value(
            file,
            &reference.type_name,
            reference.type_arguments.as_deref(),
        )?,
        AstKind::TSInterfaceHeritage(heritage) => record_value(
            file,
            &heritage.type_name,
            heritage.type_arguments.as_deref(),
        )?,
        AstKind::TSClassImplements(implements) => record_value(
            file,
            &implements.expression,
            implements.type_arguments.as_deref(),
        )?,
        AstKind::TSIndexSignature(signature) => &signature.type_annotation.type_annotation,
        AstKind::TSMappedType(mapped) => mapped.type_annotation.as_ref()?,
        _ => return None,
    };
    unsafe_value(file, value)
}

fn record_value<'a>(
    file: &File<'_>,
    name: &TSTypeName<'_>,
    arguments: Option<&'a TSTypeParameterInstantiation<'a>>,
) -> Option<&'a TSType<'a>> {
    let TSTypeName::IdentifierReference(id) = name else {
        return None;
    };
    if id.name != "Record" || !file.semantic.is_reference_to_global_variable(id) {
        return None;
    }
    arguments?.params.get(1)
}

fn unsafe_value(file: &File<'_>, ty: &TSType<'_>) -> Option<&'static str> {
    match unwrapped_type(file, ty) {
        TSType::TSUnknownKeyword(_) => Some("`unknown`"),
        TSType::TSAnyKeyword(_) => Some("`any`"),
        TSType::TSObjectKeyword(_) => Some("`object`"),
        TSType::TSTypeLiteral(literal) if literal.members.is_empty() => Some("`{}`"),
        TSType::TSUnionType(union) => union.types.iter().find_map(|t| unsafe_value(file, t)),
        // `unknown & { id: string }` has a known part; `any` swallows the rest.
        TSType::TSIntersectionType(intersection) => {
            let values: Vec<_> = intersection
                .types
                .iter()
                .map(|t| unsafe_value(file, t))
                .collect();
            if values.contains(&Some("`any`")) {
                Some("`any`")
            } else if values.contains(&None) {
                None
            } else {
                values.first().copied().flatten()
            }
        }
        _ => None,
    }
}

/// The finding's span when `kind` is the first member or declaration above the dictionary:
/// the owner's name and type (`payload: Record<string, unknown>`, without a decorator,
/// `private` or `= ...`; `Props extends Record<string, unknown>`) when the dictionary is in its
/// annotation or heritage, else the dictionary itself.
fn owner_span(kind: AstKind<'_>, dictionary: Span) -> Option<Span> {
    let (start, end) = match kind {
        AstKind::TSPropertySignature(p) => (
            p.key.span().start,
            p.type_annotation.as_ref().map(|a| a.span.end),
        ),
        AstKind::PropertyDefinition(p) => (
            p.key.span().start,
            p.type_annotation.as_ref().map(|a| a.span.end),
        ),
        AstKind::AccessorProperty(p) => (
            p.key.span().start,
            p.type_annotation.as_ref().map(|a| a.span.end),
        ),
        AstKind::FormalParameter(p) => (
            p.pattern.span().start,
            p.type_annotation.as_ref().map(|a| a.span.end),
        ),
        AstKind::VariableDeclarator(d) => {
            (d.span.start, d.type_annotation.as_ref().map(|a| a.span.end))
        }
        AstKind::TSTypeAliasDeclaration(alias) => {
            (alias.span.start, Some(alias.type_annotation.span().end))
        }
        AstKind::TSInterfaceDeclaration(interface) => {
            (interface.id.span.start, Some(dictionary.end))
        }
        AstKind::Class(class) => (
            class
                .id
                .as_ref()
                .map_or(dictionary.start, |id| id.span.start),
            Some(dictionary.end),
        ),
        AstKind::TSIndexSignature(_) => return Some(dictionary),
        _ => return None,
    };
    Some(
        end.map(|end| Span::new(start, end))
            .filter(|owned| owned.contains_inclusive(dictionary))
            .unwrap_or(dictionary),
    )
}

/// Nodes inside a type; the walk up from a dictionary stops after the first other node.
fn is_type_part(kind: AstKind<'_>) -> bool {
    matches!(
        kind,
        AstKind::TSTypeAnnotation(_)
            | AstKind::TSTypeParameterInstantiation(_)
            | AstKind::TSTypeParameter(_)
            | AstKind::TSTypeReference(_)
            | AstKind::TSUnionType(_)
            | AstKind::TSIntersectionType(_)
            | AstKind::TSArrayType(_)
            | AstKind::TSTupleType(_)
            | AstKind::TSNamedTupleMember(_)
            | AstKind::TSTypeOperator(_)
            | AstKind::TSTypeLiteral(_)
            | AstKind::TSIndexSignature(_)
            | AstKind::TSMappedType(_)
            | AstKind::TSPropertySignature(_)
            | AstKind::TSIndexedAccessType(_)
            | AstKind::TSConditionalType(_)
            | AstKind::TSFunctionType(_)
    )
}
