use std::collections::HashMap;

use kit::oxc::ast::ast::{
    BindingPattern, ClassElement, Declaration, ExportDefaultDeclarationKind, Expression, Statement,
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
/// class members) whose code calls it, each with where it starts and its name. Names are source
/// text, put on one line only for a finding.
struct Callee<'a> {
    unit: u32,
    span: Span,
    name: &'a str,
    callers: Vec<(u32, &'a str)>,
}

/// The finding when the callee sits above every caller, unless one of them is also its callee:
/// two functions that call each other cannot both come first.
fn above_callers(callees: &[Callee<'_>]) -> Vec<(Span, String, String)> {
    callees
        .iter()
        .filter_map(|callee| {
            let first = callee.callers.iter().min_by_key(|(start, _)| *start)?;
            let is_mutual = || {
                callee.callers.iter().any(|(start, _)| {
                    callees.iter().any(|other| {
                        other.unit == *start
                            && other.callers.iter().any(|(at, _)| *at == callee.unit)
                    })
                })
            };
            (callee.unit < first.0 && !is_mutual())
                .then(|| (callee.span, one_line(callee.name), one_line(first.1)))
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
        let Some(own) = unit_of(binding.span.start).map(|unit| unit.span().start) else {
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
            // Moving a hoisted `function` below a callback that runs on load is still safe.
            let is_on_load = if is_hoisted {
                deferred_by(file, node.id()).is_none()
            } else {
                runs_on_load(file, node.id())
            };
            if is_on_load {
                is_used_on_load = true;
                continue;
            }
            if unit.span().start == own {
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
            unit: own,
            span: binding.span,
            name: binding.span.source_text(file.text),
            callers,
        });
    }
    above_callers(&callees)
}

/// What makes the reference run after the module loads: the function it sits in, or the instance
/// field whose value holds it, which runs for each `new`. A decorator and a field's computed key
/// run when the class is defined: `@Column(money())`, `constructor(@Inject(token('repo')) ..)`.
fn deferred_by<'a>(file: &File<'a>, node: NodeId) -> Option<&'a AstNode<'a>> {
    let nodes = file.semantic.nodes();
    let at = nodes.get_node(node).span();
    let holds = |value: &Option<Expression<'_>>| {
        value
            .as_ref()
            .is_some_and(|value| value.span().contains_inclusive(at))
    };
    let mut in_decorator = false;
    for ancestor in nodes.ancestors(node) {
        match ancestor.kind() {
            AstKind::Decorator(_) => in_decorator = true,
            AstKind::Class(_) => in_decorator = false,
            AstKind::PropertyDefinition(field) if !field.r#static && holds(&field.value) => {
                return Some(ancestor);
            }
            AstKind::AccessorProperty(field) if !field.r#static && holds(&field.value) => {
                return Some(ancestor);
            }
            kind if kind.is_function_like() && !in_decorator => return Some(ancestor),
            _ => {}
        }
    }
    None
}

/// The reference runs while the module loads: outside any function, or in a callback that such
/// code passes to a call or calls at once (`ITEMS.map((item) => toRoute(item))`, an IIFE).
fn runs_on_load(file: &File<'_>, node: NodeId) -> bool {
    let Some(owner) = deferred_by(file, node) else {
        return true;
    };
    let call = file.semantic.nodes().parent_node(owner.id());
    matches!(
        call.kind(),
        AstKind::CallExpression(_) | AstKind::NewExpression(_)
    ) && runs_on_load(file, call.id())
}

/// The name of the top-level declaration that holds the offset: its function, class or variable.
fn unit_name<'a>(file: &File<'a>, statement: &Statement<'a>, at: u32) -> Option<&'a str> {
    let declaration = match statement {
        Statement::ExportDeclaration(export) => &export.declaration,
        Statement::ExportDefaultDeclaration(export) => {
            let id = match &export.declaration {
                ExportDefaultDeclarationKind::FunctionDeclaration(function) => function.id.as_ref(),
                ExportDefaultDeclarationKind::ClassDeclaration(class) => class.id.as_ref(),
                _ => None,
            };
            return Some(id.map_or("default", |id| id.name.as_str()));
        }
        statement => statement.as_declaration()?,
    };
    match declaration {
        Declaration::FunctionDeclaration(function) => Some(function.id.as_ref()?.name.as_str()),
        Declaration::ClassDeclaration(class) => Some(class.id.as_ref()?.name.as_str()),
        Declaration::VariableDeclaration(variables) => {
            let declarator = variables
                .declarations
                .iter()
                .find(|declarator| declarator.span.start <= at && at < declarator.span.end)?;
            Some(match &declarator.id {
                BindingPattern::BindingIdentifier(id) => id.name.as_str(),
                pattern => pattern.span().source_text(file.text),
            })
        }
        _ => None,
    }
}

/// Methods of each class and the members of the same class that call them through `this`.
fn methods(file: &File<'_>) -> Vec<(Span, String, String)> {
    let nodes = file.semantic.nodes();
    let mut callers: HashMap<NodeId, Vec<(u32, &str)>> = HashMap::new();
    let mut classes = Vec::new();
    for node in nodes.iter() {
        let call = match node.kind() {
            AstKind::CallExpression(call) => call,
            AstKind::Class(class) => {
                classes.push(class);
                continue;
            }
            _ => continue,
        };
        // Only `this.m(..)` can run a method; `local_callee` on a plain name would scan all its
        // references for nothing.
        let is_this_call = match call.callee.get_inner_expression() {
            Expression::StaticMemberExpression(member) => {
                matches!(member.object, Expression::ThisExpression(_))
            }
            Expression::PrivateFieldExpression(member) => {
                matches!(member.object, Expression::ThisExpression(_))
            }
            _ => false,
        };
        if !is_this_call {
            continue;
        }
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
    if callers.is_empty() {
        return Vec::new();
    }
    let mut found = Vec::new();
    for class in classes {
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
                    name: method.key.span().source_text(file.text),
                    callers: callers.remove(&method.node_id.get())?,
                })
            })
            .collect();
        found.extend(above_callers(&callees));
    }
    found
}

fn member_name<'a>(file: &File<'a>, kind: AstKind<'_>) -> Option<&'a str> {
    let key = match kind {
        AstKind::MethodDefinition(method) => &method.key,
        AstKind::PropertyDefinition(field) => &field.key,
        AstKind::AccessorProperty(field) => &field.key,
        _ => return None,
    };
    Some(key.span().source_text(file.text))
}
