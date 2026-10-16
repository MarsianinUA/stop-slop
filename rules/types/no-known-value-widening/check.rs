use std::collections::HashMap;
use std::fmt;

use kit::oxc::ast::MemberExpressionKind;
use kit::oxc::ast::ast::{
    AssignmentTarget, BindingPattern, Expression, ObjectPropertyKind, PropertyKey, TSAccessibility,
    TSSignature, TSType, TSTypeAnnotation, UnaryOperator,
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
        let (value, ty, writes) = match node.kind() {
            AstKind::VariableDeclarator(d) => (
                d.init.as_ref(),
                written(d.type_annotation.as_deref()),
                d.id.get_binding_identifier()
                    .and_then(|id| id.symbol_id.get())
                    .map_or(Writes::One, Writes::Variable),
            ),
            AstKind::PropertyDefinition(p) => (
                p.value.as_ref(),
                written(p.type_annotation.as_deref()),
                field_writes(
                    &p.key,
                    p.readonly || p.accessibility == Some(TSAccessibility::Private),
                ),
            ),
            AstKind::AccessorProperty(p) => (
                p.value.as_ref(),
                written(p.type_annotation.as_deref()),
                field_writes(&p.key, p.accessibility == Some(TSAccessibility::Private)),
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
                    Writes::Variable(symbol),
                )
            }
            AstKind::ArrowFunctionExpression(f) => (
                f.get_expression(),
                written(f.return_type.as_deref()),
                Writes::One,
            ),
            kind => {
                let Some((value, ty)) = type_assertion(kind) else {
                    return;
                };
                // The inner link of a chain is the finding of `no-chained-type-assertions`.
                let parent = file.semantic.nodes().parent_kind(node.id());
                if type_assertion(parent).is_some() || has_safety_comment(file, node.span()) {
                    return;
                }
                (Some(value), Some(ty), Writes::One)
            }
        };
        let (Some(value), Some(ty)) = (value, ty) else {
            return;
        };
        let Some(target) = widening_target(file, ty).filter(|&t| widens(file, value, t)) else {
            return;
        };
        let others_widen = match writes {
            Writes::One => true,
            Writes::Variable(symbol) => every_write_widens(file, symbol, target),
            Writes::Field(key) => every_field_write_widens(file, key, target),
            Writes::Elsewhere => false,
        };
        if !others_widen {
            return;
        }
        // From the name: `a: unknown = 1` and `b: unknown = 1` get different fingerprints.
        let start = match node.kind() {
            AstKind::VariableDeclarator(d) => d.span.start,
            AstKind::PropertyDefinition(p) => p.key.span().start,
            AstKind::AccessorProperty(p) => p.key.span().start,
            AstKind::AssignmentExpression(assignment) => assignment.span.start,
            _ => value.span().start,
        };
        cx.report(Span::new(start, value.span().end))
            .arg("target", target);
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

/// The other writes into what gets the value: one that is not known keeps the wide type.
enum Writes<'a> {
    One,
    Variable(SymbolId),
    /// A field that only this file writes.
    Field(&'a PropertyKey<'a>),
    /// A field that other files may write too.
    Elsewhere,
}

/// Code outside the class may write a field that is not `private`, `readonly` or `#name`.
fn field_writes<'a>(key: &'a PropertyKey<'a>, closed: bool) -> Writes<'a> {
    if closed || key.is_private_identifier() {
        Writes::Field(key)
    } else {
        Writes::Elsewhere
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

/// Every write to a member of the field's name in the file gives a known value too:
/// `this.state = input` needs the wide type.
fn every_field_write_widens(file: &File<'_>, key: &PropertyKey<'_>, target: Target) -> bool {
    let Some(name) = key.name() else {
        return false;
    };
    let private = key.is_private_identifier();
    let nodes = file.semantic.nodes();
    nodes.iter().all(|node| {
        let Some(member) = node.kind().as_member_expression_kind() else {
            return true;
        };
        let same = match member {
            MemberExpressionKind::PrivateField(member) => {
                private && name == member.field.name.as_str()
            }
            _ => match member.static_property_name() {
                Some(property) => !private && name == property.as_str(),
                // `this[key] = ..` may write any field.
                None => matches!(member.object(), Expression::ThisExpression(_)),
            },
        };
        !same
            || match assigned(file, node) {
                None => true,
                Some(Some(value)) => widens(file, value, target),
                Some(None) => false,
            }
    })
}

/// What a write puts into the member at `node`, also through `!`, `as`, `<T>` and
/// `satisfies`: `None` for a read, `Some(None)` for a write without one value (`+=`, `++`,
/// `delete`, destructuring, a `for` target).
fn assigned<'a>(file: &File<'a>, node: &AstNode<'a>) -> Option<Option<&'a Expression<'a>>> {
    let target = outer_assertion(file, node);
    let span = target.span();
    let write = match file.semantic.nodes().parent_kind(target.id()) {
        AstKind::AssignmentExpression(assignment) if assignment.left.span() == span => {
            return Some(assignment.operator.is_assign().then_some(&assignment.right));
        }
        AstKind::UpdateExpression(_)
        | AstKind::ArrayAssignmentTarget(_)
        | AstKind::AssignmentTargetRest(_) => true,
        AstKind::UnaryExpression(unary) => unary.operator == UnaryOperator::Delete,
        AstKind::AssignmentTargetPropertyProperty(property) => property.binding.span() == span,
        AstKind::AssignmentTargetWithDefault(default) => default.binding.span() == span,
        AstKind::ForOfStatement(statement) => statement.left.span() == span,
        AstKind::ForInStatement(statement) => statement.left.span() == span,
        _ => false,
    };
    write.then_some(None)
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
