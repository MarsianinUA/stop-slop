use std::borrow::Cow;
use std::cmp::Reverse;

use kit::LineIndex;
use kit::oxc::allocator::Allocator;
use kit::oxc::ast::ast::PropertyKind;
use kit::oxc::semantic::{AstNodes, NodeId, SemanticBuilder};
use kit::oxc::span::{GetSpan, SourceType, Span};
use kit::prelude::AstKind;

use crate::{Engine, FunctionSpan, SourceFile, finding, run, text};

impl Engine {
    /// Every function and arrow of the file that has a body, outermost first; empty when the
    /// file does not parse. V8 puts the start of a function in different places (a declaration
    /// at `function` or at its `(`, an arrow at its parameters, a method at its name), so a
    /// position of V8 is matched by the span it falls in, the innermost one. For that a method
    /// spans its key too, and an exported declaration its `export`, where the source map of
    /// `tsc` puts the start that coverage reports.
    pub fn functions(file: &SourceFile) -> Vec<FunctionSpan> {
        let Some((text, _)) = text::decode(&file.bytes) else {
            return Vec::new();
        };
        let Ok(source_type) = SourceType::from_path(&file.path) else {
            return Vec::new();
        };
        let allocator = Allocator::default();
        let Ok(program) = run::parse(&allocator, text, source_type) else {
            return Vec::new();
        };
        let semantic = SemanticBuilder::new()
            .with_build_nodes(true)
            .build(&program)
            .semantic;
        let nodes = semantic.nodes();
        let mut found: Vec<(Span, Option<String>)> = nodes
            .iter()
            .filter(|node| has_body(node.kind()))
            .map(|node| {
                let id = node.id();
                let name = own_name(nodes, id).or_else(|| {
                    nodes
                        .ancestor_ids(id)
                        .filter(|&ancestor| has_body(nodes.kind(ancestor)))
                        .find_map(|ancestor| own_name(nodes, ancestor))
                });
                (reach(nodes, id), name.map(Cow::into_owned))
            })
            .collect();
        // A function in the computed key of a method comes before the method's own node.
        found.sort_by_key(|(span, _)| (span.start, Reverse(span.end)));
        let lines = LineIndex::new(text);
        found
            .into_iter()
            .map(|(span, name)| FunctionSpan {
                name,
                start: lines.position(span.start),
                end: lines.position(span.end),
            })
            .collect()
    }
}

/// An overload, a `declare` and an abstract method have no body and never run.
fn has_body(kind: AstKind<'_>) -> bool {
    match kind {
        AstKind::Function(function) => function.body.is_some(),
        AstKind::ArrowFunctionExpression(_) => true,
        _ => false,
    }
}

/// The name of the function itself, or of the method, property or variable that holds it.
fn own_name<'a>(nodes: &AstNodes<'a>, id: NodeId) -> Option<Cow<'a, str>> {
    finding::function_name(nodes.kind(id)).or_else(|| finding::function_name(nodes.parent_kind(id)))
}

fn reach(nodes: &AstNodes<'_>, id: NodeId) -> Span {
    match nodes.parent_kind(id) {
        AstKind::MethodDefinition(method) => method.span,
        AstKind::ObjectProperty(property)
            if property.method
                || matches!(property.kind, PropertyKind::Get | PropertyKind::Set) =>
        {
            property.span
        }
        AstKind::ExportDeclaration(export) => export.span,
        AstKind::ExportDefaultDeclaration(export) => export.span,
        _ => nodes.get_node(id).span(),
    }
}
