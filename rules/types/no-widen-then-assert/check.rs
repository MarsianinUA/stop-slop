use std::collections::HashSet;

use kit::oxc::ast::ast::{
    BindingPattern, Expression, TSSignature, TSType, VariableDeclarationKind, VariableDeclarator,
};
use kit::oxc::semantic::{NodeId, SymbolId};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::TSAsExpression, AstType::TSTypeAssertion];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let Some((Expression::Identifier(id), asserted)) = type_assertion(node.kind()) else {
            return;
        };
        if broad(file, asserted).is_some() {
            return;
        }
        let Some(symbol) = file
            .semantic
            .scoping()
            .get_reference(id.reference_id())
            .symbol_id()
        else {
            return;
        };
        let Some((declaration, declarator, init)) = const_declarator(file, symbol) else {
            return;
        };
        if node.span().start <= declarator.span.end {
            return;
        }
        let declared = declarator
            .type_annotation
            .as_ref()
            .and_then(|a| broad(file, &a.type_annotation));
        // `const loose = settings as unknown`: the value before the cast holds the evidence.
        let widening_cast =
            expression_assertion(init).and_then(|(value, ty)| Some((value, broad(file, ty)?)));
        let original = widening_cast.map_or(init, |(value, _)| value);
        let Some(kind) = declared.or(widening_cast.map(|(_, kind)| kind)) else {
            return;
        };
        let function = function_of(file, declaration);
        if function_of(file, node.id()) != function {
            return;
        }
        let Some(evidence) = evidence(file, original, function, &mut HashSet::from([symbol]))
        else {
            return;
        };
        let narrower = match kind {
            Broad::Top => true,
            _ if evidence.is_some_and(|ty| same_text(file, ty, asserted)) => true,
            Broad::Object => is_object_type(file, asserted),
            Broad::Record => is_narrower_record(file, asserted),
        };
        if narrower && !has_safety_comment(file, node.span()) {
            cx.report(node.span()).arg("name", id.name);
        }
    }
}

#[derive(Clone, Copy)]
enum Broad {
    Top,
    Object,
    /// `Record<string, unknown>`, `{ [key: string]: any }`.
    Record,
}

fn broad(file: &File<'_>, ty: &TSType<'_>) -> Option<Broad> {
    match unwrapped_type(file, ty) {
        TSType::TSUnknownKeyword(_) | TSType::TSAnyKeyword(_) => Some(Broad::Top),
        TSType::TSObjectKeyword(_) => Some(Broad::Object),
        TSType::TSTypeLiteral(literal) => match literal.members.as_slice() {
            [TSSignature::TSIndexSignature(signature)]
                if is_open_key(file, &signature.parameter.type_annotation.type_annotation)
                    && is_top(file, &signature.type_annotation.type_annotation) =>
            {
                Some(Broad::Record)
            }
            _ => None,
        },
        ty => {
            let params = &lib_type(file, ty, "Record")?
                .type_arguments
                .as_ref()?
                .params;
            match params.as_slice() {
                [key, value] if is_open_key(file, key) && is_top(file, value) => {
                    Some(Broad::Record)
                }
                _ => None,
            }
        }
    }
}

fn is_top(file: &File<'_>, ty: &TSType<'_>) -> bool {
    matches!(
        unwrapped_type(file, ty),
        TSType::TSUnknownKeyword(_) | TSType::TSAnyKeyword(_)
    )
}

/// What shows the original value's type: `Some(Some(ty))` - a written type, `Some(None)` -
/// a value whose type its syntax shows, `None` - nothing (a call, a parameter of another
/// function, a value that came in as `unknown`).
fn evidence<'a>(
    file: &File<'a>,
    value: &'a Expression<'a>,
    function: Option<NodeId>,
    seen: &mut HashSet<SymbolId>,
) -> Option<Option<&'a TSType<'a>>> {
    if let Some((_, ty)) = expression_assertion(value) {
        return broad(file, ty).is_none().then_some(Some(ty));
    }
    match value {
        Expression::Identifier(id) => {
            let symbol = file
                .semantic
                .scoping()
                .get_reference(id.reference_id())
                .symbol_id()?;
            if !seen.insert(symbol) {
                return None;
            }
            let declaration = file.semantic.scoping().symbol_declaration(symbol);
            let annotation = match file.semantic.nodes().kind(declaration) {
                AstKind::VariableDeclarator(d) if is_single(&d.id) => d.type_annotation.as_ref(),
                AstKind::FormalParameter(p) if is_single(&p.pattern) => p.type_annotation.as_ref(),
                _ => return None,
            };
            if function_of(file, declaration) != function {
                return None;
            }
            if let Some(annotation) = annotation {
                let ty = &annotation.type_annotation;
                return broad(file, ty).is_none().then_some(Some(ty));
            }
            let (_, _, init) = const_declarator(file, symbol)?;
            evidence(file, init, function, seen)
        }
        Expression::NullLiteral(_) => Some(None),
        _ => is_known_value(file, value).then_some(None),
    }
}

fn is_single(pattern: &BindingPattern<'_>) -> bool {
    matches!(pattern, BindingPattern::BindingIdentifier(_))
}

/// `const name = ...` with nothing writing to `name` later.
fn const_declarator<'a>(
    file: &File<'a>,
    symbol: SymbolId,
) -> Option<(NodeId, &'a VariableDeclarator<'a>, &'a Expression<'a>)> {
    let scoping = file.semantic.scoping();
    let nodes = file.semantic.nodes();
    let declaration = scoping.symbol_declaration(symbol);
    let (AstKind::VariableDeclarator(declarator), AstKind::VariableDeclaration(statement)) =
        (nodes.kind(declaration), nodes.parent_kind(declaration))
    else {
        return None;
    };
    let constant = statement.kind == VariableDeclarationKind::Const
        && is_single(&declarator.id)
        && !scoping
            .get_resolved_references(symbol)
            .any(|r| r.is_write());
    let init = declarator.init.as_ref().filter(|_| constant)?;
    Some((declaration, declarator, init))
}

fn function_of(file: &File<'_>, id: NodeId) -> Option<NodeId> {
    file.semantic.nodes().ancestors(id).find_map(|node| {
        matches!(
            node.kind(),
            AstKind::Function(_) | AstKind::ArrowFunctionExpression(_)
        )
        .then(|| node.id())
    })
}

fn same_text(file: &File<'_>, a: &TSType<'_>, b: &TSType<'_>) -> bool {
    let text = |ty: &TSType<'_>| -> String {
        ty.span()
            .source_text(file.text)
            .split_whitespace()
            .collect()
    };
    text(a) == text(b)
}

/// A type that only objects have: an array, tuple, function, class, mapped type, non-empty
/// literal, or an intersection of such.
fn is_object_type(file: &File<'_>, ty: &TSType<'_>) -> bool {
    match unwrapped_type(file, ty) {
        TSType::TSArrayType(_)
        | TSType::TSTupleType(_)
        | TSType::TSFunctionType(_)
        | TSType::TSConstructorType(_)
        | TSType::TSMappedType(_)
        | TSType::TSObjectKeyword(_) => true,
        TSType::TSTypeLiteral(literal) => !literal.members.is_empty(),
        TSType::TSIntersectionType(intersection) => {
            intersection.types.iter().all(|t| is_object_type(file, t))
        }
        _ => false,
    }
}

/// A literal with a named member, or lib `Record<K, V>` with `V` not `unknown` or `any`.
fn is_narrower_record(file: &File<'_>, ty: &TSType<'_>) -> bool {
    match unwrapped_type(file, ty) {
        TSType::TSTypeLiteral(literal) => literal
            .members
            .iter()
            .any(|m| !matches!(m, TSSignature::TSIndexSignature(_))),
        ty => lib_type(file, ty, "Record")
            .and_then(|record| record.type_arguments.as_ref()?.params.get(1))
            .is_some_and(|value| !is_top(file, value)),
    }
}
