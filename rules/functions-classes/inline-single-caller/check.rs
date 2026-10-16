use std::borrow::Cow;

use kit::oxc::ast::MemberExpressionKind;
use kit::oxc::ast::ast::{
    BindingPattern, Expression, Function, MethodDefinitionKind, TSType, TSTypeAnnotation,
};
use kit::oxc::semantic::{NodeId, SymbolId};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let nodes = file.semantic.nodes();
        let scoping = file.semantic.scoping();
        let mut functions: Vec<(SymbolId, Span)> = Vec::new();
        let mut methods: Vec<Method> = Vec::new();
        let mut uses: Vec<Use> = Vec::new();
        // `this[key]` may reach any member of the class.
        let mut dynamic: Vec<Span> = Vec::new();
        for node in nodes.iter() {
            match node.kind() {
                AstKind::Function(function)
                    if function.is_declaration() && !function.declare && keeps_shape(function) =>
                {
                    if let Some(symbol) = function.id.as_ref().and_then(|id| id.symbol_id.get()) {
                        functions.push((symbol, function.span));
                    }
                }
                AstKind::VariableDeclarator(declarator) => {
                    let BindingPattern::BindingIdentifier(id) = &declarator.id else {
                        continue;
                    };
                    let value = match declarator
                        .init
                        .as_ref()
                        .map(Expression::get_inner_expression)
                    {
                        Some(Expression::FunctionExpression(function)) if keeps_shape(function) => {
                            function.span
                        }
                        Some(Expression::ArrowFunctionExpression(arrow))
                            if !is_guard(arrow.return_type.as_deref()) =>
                        {
                            arrow.span
                        }
                        _ => continue,
                    };
                    if let Some(symbol) = id.symbol_id.get() {
                        functions.push((symbol, value));
                    }
                }
                AstKind::MethodDefinition(method)
                    if method.kind == MethodDefinitionKind::Method
                        && keeps_shape(&method.value) =>
                {
                    let Some((_, key)) = private_member(file, node) else {
                        continue;
                    };
                    // The method's parent is the class body.
                    let AstKind::Class(class) = nodes.parent_kind(nodes.parent_id(node.id()))
                    else {
                        continue;
                    };
                    methods.push(Method {
                        name: key.source_text(file.text),
                        key,
                        span: method.span,
                        class: class.span,
                    });
                }
                // `const { step } = this` takes the method without a member expression.
                AstKind::BindingProperty(property) => {
                    uses.extend(Use::new(node, property.key.static_name()))
                }
                AstKind::AssignmentTargetPropertyProperty(property) => {
                    uses.extend(Use::new(node, property.name.static_name()));
                }
                AstKind::AssignmentTargetPropertyIdentifier(property) => {
                    uses.extend(Use::new(node, Some(property.binding.name.as_str().into())));
                }
                kind => {
                    let Some(member) = kind.as_member_expression_kind() else {
                        continue;
                    };
                    let name = match &member {
                        MemberExpressionKind::PrivateField(member) => {
                            Some(member.field.span.source_text(file.text).into())
                        }
                        _ => member.static_property_name().map(Cow::from),
                    };
                    match name {
                        None if matches!(
                            member.object().get_inner_expression(),
                            Expression::ThisExpression(_)
                        ) =>
                        {
                            dynamic.push(node.span());
                        }
                        name => uses.extend(Use::new(node, name)),
                    }
                }
            }
        }

        for (symbol, function) in functions {
            let mut references = scoping.get_resolved_references(symbol);
            let (Some(reference), None) = (references.next(), references.next()) else {
                continue;
            };
            let Some(line) = step_call(file, reference.node_id(), function) else {
                continue;
            };
            if is_own_name(file, symbol) && !is_exported(file, symbol) {
                cx.report(scoping.symbol_span(symbol))
                    .arg("name", scoping.symbol_name(symbol))
                    .arg("line", line);
            }
        }

        for method in methods {
            if dynamic
                .iter()
                .any(|&span| method.class.contains_inclusive(span))
            {
                continue;
            }
            let mut method_uses = uses
                .iter()
                .filter(|u| u.name == method.name && method.class.contains_inclusive(u.span));
            let (Some(only), None) = (method_uses.next(), method_uses.next()) else {
                continue;
            };
            if let Some(line) = step_call(file, only.id, method.span) {
                cx.report(method.key)
                    .arg("name", method.name)
                    .arg("line", line);
            }
        }
    }
}

struct Method<'a> {
    /// `prepare`, `#finish`.
    name: &'a str,
    key: Span,
    span: Span,
    class: Span,
}

/// A member reached by name: `this.m`, `C.m`, `other.m`, `this.#m`, `this['m']`, or a key in a
/// destructuring pattern. The type of the object is not seen: inside the class any of them may
/// reach its private method (`other.total()` on another instance).
struct Use<'a> {
    name: Cow<'a, str>,
    span: Span,
    id: NodeId,
}

impl<'a> Use<'a> {
    fn new(node: &AstNode<'a>, name: Option<Cow<'a, str>>) -> Option<Self> {
        Some(Use {
            name: name?,
            span: node.span(),
            id: node.id(),
        })
    }
}

/// Inlining a generator loses its iterator, and a type guard or `asserts` its narrowing.
fn keeps_shape(function: &Function<'_>) -> bool {
    function.body.is_some() && !function.generator && !is_guard(function.return_type.as_deref())
}

fn is_guard(return_type: Option<&TSTypeAnnotation<'_>>) -> bool {
    return_type.is_some_and(|ty| matches!(ty.type_annotation, TSType::TSTypePredicate(_)))
}

/// The line of the call when the node is its callee, outside the function itself (not recursion),
/// and the call is a step of its caller: through `await` the whole statement, the whole value of
/// a variable or of a `return`, inside a function that is not a callback. A call inside an
/// expression (`if (isEligible(x))`, `a + fee(b)`) computes a value, which a name usually gives.
fn step_call(file: &File<'_>, id: NodeId, function: Span) -> Option<u32> {
    let nodes = file.semantic.nodes();
    let callee = nodes.get_node(id);
    let call = nodes.parent_node(id);
    if !matches!(call.kind(), AstKind::CallExpression(c) if c.callee.span() == callee.span())
        || function.contains_inclusive(call.span())
    {
        return None;
    }
    let mut step = call;
    loop {
        let parent = nodes.parent_node(step.id());
        match parent.kind() {
            AstKind::AwaitExpression(_) => step = parent,
            AstKind::ExpressionStatement(_) | AstKind::ReturnStatement(_) => break,
            AstKind::VariableDeclarator(declarator)
                if declarator
                    .init
                    .as_ref()
                    .is_some_and(|init| init.span() == step.span()) =>
            {
                break;
            }
            _ => return None,
        }
    }
    // At the top of the module (`bootstrap();`) there is no caller to inline into.
    let caller = nodes
        .ancestors(step.id())
        .find(|ancestor| ancestor.kind().is_function_like())?;
    if is_callback(file, caller) {
        return None;
    }
    Some(file.lines.position(call.span().start).line)
}

/// A function handed to someone else to call: an argument of a call or `new`, the value of a JSX
/// attribute or of an object property (as in `no-single-letter-name`).
fn is_callback(file: &File<'_>, function: &AstNode<'_>) -> bool {
    let nodes = file.semantic.nodes();
    let span = function.span();
    match nodes.parent_kind(function.id()) {
        AstKind::CallExpression(outer) => outer.callee.span() != span,
        AstKind::NewExpression(outer) => outer.callee.span() != span,
        AstKind::JSXExpressionContainer(_) => matches!(
            nodes.parent_kind(nodes.parent_id(function.id())),
            AstKind::JSXAttribute(_)
        ),
        AstKind::ObjectProperty(property) => property.value.span() == span,
        _ => false,
    }
}
