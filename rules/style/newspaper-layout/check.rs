use std::collections::HashMap;

use kit::oxc::ast::ast::{
    BindingPattern, ClassElement, Declaration, ExportDefaultDeclarationKind, Statement,
};
use kit::oxc::semantic::NodeId;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let mut found = top_functions(file);
        found.extend(methods(file));
        for (span, name, caller) in found {
            cx.report(span).arg("name", name).arg("caller", caller);
        }
    }
}

/// A function or method that can move: where it starts, and the units (top-level statements or
/// class members) whose code calls it, each with where it starts and its name.
struct Callee {
    unit: u32,
    span: Span,
    name: String,
    callers: Vec<(u32, String)>,
}

/// The finding when the callee sits above every caller, unless one of them is also its callee:
/// two functions that call each other cannot both come first.
fn above_callers(callees: &[Callee]) -> Vec<(Span, String, String)> {
    callees
        .iter()
        .filter_map(|callee| {
            let first = callee.callers.iter().min_by_key(|(start, _)| *start)?;
            let is_mutual = callee.callers.iter().any(|(start, _)| {
                callees.iter().any(|other| {
                    other.unit == *start && other.callers.iter().any(|(at, _)| *at == callee.unit)
                })
            });
            (callee.unit < first.0 && !is_mutual)
                .then(|| (callee.span, callee.name.clone(), first.1.clone()))
        })
        .collect()
}

/// Functions declared at the top of the file and the top-level declarations that call them.
fn top_functions(file: &File<'_>) -> Vec<(Span, String, String)> {
    let nodes = file.semantic.nodes();
    let scoping = file.semantic.scoping();
    let body = &nodes.program().body;
    let unit_of = |at: u32| {
        let index = body.partition_point(|statement| statement.span().end <= at);
        body.get(index)
            .filter(|statement| statement.span().start <= at)
    };
    let mut callees = Vec::new();
    for binding in top_bindings(file) {
        let [symbol] = binding.symbols[..] else {
            continue;
        };
        if binding.kind != TopKind::Function {
            continue;
        }
        let Some(own) = unit_of(binding.span.start) else {
            continue;
        };
        // A `const` function is not hoisted: code that runs on load must find it above.
        let is_hoisted = binding.init.is_none();
        let mut callers = Vec::new();
        let mut is_used_on_load = false;
        for reference in scoping.get_resolved_references(symbol) {
            let node = nodes.get_node(reference.node_id());
            if reference.is_type()
                || matches!(nodes.parent_kind(node.id()), AstKind::ExportSpecifier(_))
            {
                continue;
            }
            let Some(unit) = unit_of(node.span().start) else {
                continue;
            };
            if !runs_later(file, node.id()) {
                is_used_on_load = true;
                continue;
            }
            if unit.span() == own.span() {
                continue;
            }
            if let Some(name) = unit_name(file, unit, node.span().start) {
                callers.push((unit.span().start, name));
            }
        }
        if is_used_on_load && !is_hoisted {
            continue;
        }
        callees.push(Callee {
            unit: own.span().start,
            span: binding.span,
            name: one_line(binding.span.source_text(file.text)),
            callers,
        });
    }
    above_callers(&callees)
}

/// The reference sits in code that runs after the module loads: inside a function, or in an
/// instance field's value, which runs for each `new`.
fn runs_later(file: &File<'_>, node: NodeId) -> bool {
    file.semantic
        .nodes()
        .ancestor_kinds(node)
        .any(|kind| match kind {
            AstKind::PropertyDefinition(field) => !field.r#static,
            AstKind::AccessorProperty(field) => !field.r#static,
            kind => kind.is_function_like(),
        })
}

/// The name of the top-level declaration that holds the offset: its function, class or variable.
fn unit_name(file: &File<'_>, statement: &Statement<'_>, at: u32) -> Option<String> {
    let declaration = match statement {
        Statement::ExportDeclaration(export) => &export.declaration,
        Statement::ExportDefaultDeclaration(export) => {
            let id = match &export.declaration {
                ExportDefaultDeclarationKind::FunctionDeclaration(function) => function.id.as_ref(),
                ExportDefaultDeclarationKind::ClassDeclaration(class) => class.id.as_ref(),
                _ => None,
            };
            return Some(id.map_or("default", |id| id.name.as_str()).to_string());
        }
        statement => statement.as_declaration()?,
    };
    match declaration {
        Declaration::FunctionDeclaration(function) => Some(function.id.as_ref()?.name.to_string()),
        Declaration::ClassDeclaration(class) => Some(class.id.as_ref()?.name.to_string()),
        Declaration::VariableDeclaration(variables) => {
            let declarator = variables
                .declarations
                .iter()
                .find(|declarator| declarator.span.start <= at && at < declarator.span.end)?;
            Some(match &declarator.id {
                BindingPattern::BindingIdentifier(id) => id.name.to_string(),
                pattern => one_line(pattern.span().source_text(file.text)),
            })
        }
        _ => None,
    }
}

/// Methods of each class and the members of the same class that call them through `this`.
fn methods(file: &File<'_>) -> Vec<(Span, String, String)> {
    let nodes = file.semantic.nodes();
    let mut callers: HashMap<NodeId, Vec<(u32, String)>> = HashMap::new();
    for node in nodes.iter() {
        let AstKind::CallExpression(call) = node.kind() else {
            continue;
        };
        let Some(function) = local_callee(file, call) else {
            continue;
        };
        let method = nodes.parent_node(function.id());
        if !matches!(method.kind(), AstKind::MethodDefinition(_)) {
            continue;
        }
        let class_body = nodes.parent_id(method.id());
        let Some(member) = nodes
            .ancestors(node.id())
            .find(|ancestor| nodes.parent_id(ancestor.id()) == class_body)
        else {
            continue;
        };
        if member.id() == method.id() {
            continue;
        }
        if let Some(name) = member_name(file, member.kind()) {
            callers
                .entry(method.id())
                .or_default()
                .push((member.span().start, name));
        }
    }
    let mut found = Vec::new();
    for node in nodes.iter() {
        let AstKind::Class(class) = node.kind() else {
            continue;
        };
        // `local_callee` runs only a method with a body through `this`: not a getter, a static
        // method or an overload signature.
        let callees: Vec<Callee> = class
            .body
            .body
            .iter()
            .filter_map(|element| {
                let ClassElement::MethodDefinition(method) = element else {
                    return None;
                };
                Some(Callee {
                    unit: method.span.start,
                    span: method.key.span(),
                    name: one_line(method.key.span().source_text(file.text)),
                    callers: callers.remove(&method.node_id.get())?,
                })
            })
            .collect();
        found.extend(above_callers(&callees));
    }
    found
}

/// The name of a class member: its key, or `constructor`.
fn member_name(file: &File<'_>, kind: AstKind<'_>) -> Option<String> {
    let key = match kind {
        AstKind::MethodDefinition(method) => &method.key,
        AstKind::PropertyDefinition(field) => &field.key,
        AstKind::AccessorProperty(field) => &field.key,
        _ => return None,
    };
    Some(one_line(key.span().source_text(file.text)))
}
