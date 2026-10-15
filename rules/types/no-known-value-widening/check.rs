use std::collections::HashMap;
use std::fmt;

use kit::oxc::ast::ast::{
    AssignmentTarget, BindingPattern, Expression, ObjectPropertyKind, TSSignature, TSType,
    TSTypeAnnotation,
};
use kit::oxc::semantic::SymbolId;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[
        AstType::VariableDeclarator,
        AstType::PropertyDefinition,
        AstType::AccessorProperty,
        AstType::AssignmentExpression,
        AstType::ArrowFunctionExpression,
        AstType::TSAsExpression,
        AstType::TSTypeAssertion,
    ];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        // `binding`: the variable whose every write must be known, not only this one.
        let (value, ty, binding) = match node.kind() {
            AstKind::VariableDeclarator(d) => (
                d.init.as_ref(),
                written(d.type_annotation.as_deref()),
                d.id.get_binding_identifier()
                    .and_then(|id| id.symbol_id.get()),
            ),
            AstKind::PropertyDefinition(p) => (
                p.value.as_ref(),
                written(p.type_annotation.as_deref()),
                None,
            ),
            AstKind::AccessorProperty(p) => (
                p.value.as_ref(),
                written(p.type_annotation.as_deref()),
                None,
            ),
            AstKind::AssignmentExpression(assignment) if assignment.operator.is_assign() => {
                let AssignmentTarget::AssignmentTargetIdentifier(id) = &assignment.left else {
                    return;
                };
                let scoping = file.semantic.scoping();
                let Some(symbol) = scoping.get_reference(id.reference_id()).symbol_id() else {
                    return;
                };
                let declaration = scoping.symbol_declaration(symbol);
                let AstKind::VariableDeclarator(d) = file.semantic.nodes().kind(declaration) else {
                    return;
                };
                // `let { a }: T`: `T` is the type of the whole pattern, not of `a`.
                if !matches!(d.id, BindingPattern::BindingIdentifier(_)) {
                    return;
                }
                (
                    Some(&assignment.right),
                    written(d.type_annotation.as_deref()),
                    Some(symbol),
                )
            }
            AstKind::ArrowFunctionExpression(f) => {
                (f.get_expression(), written(f.return_type.as_deref()), None)
            }
            kind => {
                let Some((value, ty)) = type_assertion(kind) else {
                    return;
                };
                // The inner link of a chain is the finding of `no-chained-type-assertions`.
                let parent = file.semantic.nodes().parent_kind(node.id());
                if type_assertion(parent).is_some() || has_safety_comment(file, node.span()) {
                    return;
                }
                (Some(value), Some(ty), None)
            }
        };
        let (Some(value), Some(ty)) = (value, ty) else {
            return;
        };
        let Some(target) = widening_target(file, ty).filter(|&t| widens(file, value, t)) else {
            return;
        };
        if binding.is_none_or(|symbol| every_write_widens(file, symbol, target)) {
            cx.report(value.span()).arg("target", target);
        }
    }

    /// A function with a wide return type reports only when every `return` gives a known
    /// value: `return value` next to `return 'null'` is a normalizer, not a widening.
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let nodes = file.semantic.nodes();
        let mut returns: HashMap<_, (Target, Vec<Option<&Expression<'_>>>)> = HashMap::new();
        for node in nodes.iter() {
            let AstKind::ReturnStatement(statement) = node.kind() else {
                continue;
            };
            let Some((function, return_type)) =
                nodes.ancestors(node.id()).find_map(|n| match n.kind() {
                    AstKind::Function(f) => Some((n.id(), f.return_type.as_deref())),
                    AstKind::ArrowFunctionExpression(f) => Some((n.id(), f.return_type.as_deref())),
                    _ => None,
                })
            else {
                continue;
            };
            let Some(target) = written(return_type).and_then(|t| widening_target(file, t)) else {
                continue;
            };
            returns
                .entry(function)
                .or_insert_with(|| (target, Vec::new()))
                .1
                .push(statement.argument.as_ref());
        }
        let mut found = Vec::new();
        for (target, values) in returns.into_values() {
            if values
                .iter()
                .all(|value| value.is_some_and(|v| widens(file, v, target)))
            {
                found.extend(values.into_iter().flatten().map(|v| (v.span(), target)));
            }
        }
        found.sort_by_key(|(span, _)| span.start);
        for (span, target) in found {
            cx.report(span).arg("target", target);
        }
    }
}

#[derive(Clone, Copy)]
enum Target {
    Unknown,
    Object,
    Dictionary,
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Target::Unknown => "`unknown`",
            Target::Object => "`object`",
            Target::Dictionary => "an open dictionary",
        })
    }
}

fn written<'a>(annotation: Option<&'a TSTypeAnnotation<'a>>) -> Option<&'a TSType<'a>> {
    annotation.map(|a| &a.type_annotation)
}

/// `unknown`, `object`, or a dictionary that takes any key: lib `Record<string, X>`,
/// `{ [key: string]: X }`, `{ [K in string]: X }`.
fn widening_target(file: &File<'_>, ty: &TSType<'_>) -> Option<Target> {
    match unwrapped_type(file, ty) {
        TSType::TSUnknownKeyword(_) => Some(Target::Unknown),
        TSType::TSObjectKeyword(_) => Some(Target::Object),
        TSType::TSTypeLiteral(literal)
            if literal
                .members
                .iter()
                .any(|m| matches!(m, TSSignature::TSIndexSignature(_))) =>
        {
            Some(Target::Dictionary)
        }
        TSType::TSMappedType(mapped) if is_open_key(file, &mapped.constraint) => {
            Some(Target::Dictionary)
        }
        ty => {
            let key = lib_type(file, ty, "Record")?
                .type_arguments
                .as_ref()?
                .params
                .first()?;
            is_open_key(file, key).then_some(Target::Dictionary)
        }
    }
}

/// The initializer and every `=` of a `let` or `var` give known values. Any other write needs
/// the wide annotation: `let failure: unknown = 'none'; failure = error;`.
fn every_write_widens(file: &File<'_>, symbol: SymbolId, target: Target) -> bool {
    let nodes = file.semantic.nodes();
    let scoping = file.semantic.scoping();
    let init = match nodes.kind(scoping.symbol_declaration(symbol)) {
        AstKind::VariableDeclarator(d) => d.init.as_ref(),
        _ => None,
    };
    init.is_none_or(|value| widens(file, value, target))
        && scoping
            .get_resolved_references(symbol)
            .filter(|r| r.is_write())
            .all(|r| match nodes.parent_kind(r.node_id()) {
                AstKind::AssignmentExpression(a) if a.operator.is_assign() => {
                    widens(file, &a.right, target)
                }
                _ => false,
            })
}

/// An empty `{}` put into a dictionary is an accumulator that gets filled later; `{ ...env }`
/// and `{ [key]: 1 }` already have a dictionary type. A value cast to a wide type is the cast's
/// finding: `const v: unknown = { a: 1 } as unknown` reports once.
fn widens(file: &File<'_>, value: &Expression<'_>, target: Target) -> bool {
    if expression_assertion(value).is_some_and(|(_, ty)| widening_target(file, ty).is_some()) {
        return false;
    }
    let unnamed = matches!(
        value.get_inner_expression(),
        Expression::ObjectExpression(object) if object.properties.iter().all(|p| match p {
            ObjectPropertyKind::SpreadProperty(_) => true,
            ObjectPropertyKind::ObjectProperty(p) => p.computed,
        })
    );
    is_known_value(file, value) && !(matches!(target, Target::Dictionary) && unnamed)
}
