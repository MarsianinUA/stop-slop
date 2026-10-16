use kit::Facts;
use kit::oxc::ast::ast::{
    BindingPattern, CallExpression, ChainElement, Class, ClassElement, Expression, FormalParameter,
    FormalParameters, MethodDefinitionKind, Statement, TSType,
};
use kit::oxc::semantic::{NodeId, SymbolId};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    names: Vec<String>,
    types: Vec<String>,
}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::Function, AstType::ArrowFunctionExpression];
    const USES: &'static [Fact] = &[Fact::Packages, Fact::Aliases];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let facts = cx.facts();
        let Some(params) = params_of(node) else {
            return;
        };
        if params.items.is_empty()
            || is_ambient(file, node.id())
            || is_signature_given(file, node)
            || is_di_constructor(file, node)
            || is_wrapper(node)
        {
            return;
        }
        for parameter in &params.items {
            // A parameter property is a field; a decorated parameter is filled by the framework.
            if parameter.has_modifier() || !parameter.decorators.is_empty() {
                continue;
            }
            let BindingPattern::BindingIdentifier(binding) = &parameter.pattern else {
                continue;
            };
            if self.is_context(file, binding.name.as_str(), parameter) {
                continue;
            }
            let Some(symbol) = binding.symbol_id.get() else {
                continue;
            };
            let next = steps(file, facts, symbol, node.id()).and_then(|steps| {
                steps
                    .into_iter()
                    .find_map(|(step, index)| next_step(file, facts, step, index))
            });
            if let Some(next) = next {
                cx.report(parameter.span)
                    .arg("name", binding.name.as_str())
                    .arg("next", next);
            }
        }
    }
}

impl Check {
    /// Context that crosses layers on purpose: a name from `names`, or a type from `types`, also
    /// with `null` / `undefined`. The type is matched by its name without type arguments
    /// (`LiveApplyMetricContext<Stage>` -> `LiveApplyMetricContext`) and with its namespace, so
    /// a domain `Transaction` does not match `Knex.Transaction`.
    fn is_context(&self, file: &File<'_>, name: &str, parameter: &FormalParameter<'_>) -> bool {
        self.names.iter().any(|context| context == name)
            || parameter
                .type_annotation
                .as_ref()
                .is_some_and(|annotation| {
                    is_nullable(&annotation.type_annotation, |ty| {
                        matches!(ty, TSType::TSTypeReference(reference)
                        if self.types.iter().any(|context| {
                            context == reference.type_name.span().source_text(file.text)
                        }))
                    })
                })
    }
}

/// The calls the parameter of `function` goes to, with its argument index, when every use of it
/// is an argument of a step: a function of this file (`local_callee`) or `is_step`. A recursive call is not a step of the chain and is left
/// out: `visitAll(order.children, seen)`. `None` when it has no use or another one, or only
/// recursive calls: an operation on the value is reading it.
fn steps<'a>(
    file: &File<'a>,
    facts: &dyn Facts,
    symbol: SymbolId,
    function: NodeId,
) -> Option<Vec<(&'a CallExpression<'a>, usize)>> {
    let nodes = file.semantic.nodes();
    let mut calls = Vec::new();
    for reference in file.semantic.scoping().get_resolved_references(symbol) {
        let argument = outer_assertion(file, nodes.get_node(reference.node_id()));
        let AstKind::CallExpression(call) = nodes.parent_kind(argument.id()) else {
            return None;
        };
        let index = call
            .arguments
            .iter()
            .position(|it| it.span() == argument.span())?;
        let callee = local_callee(file, call);
        if callee.is_none() && !is_step(file, facts, call) {
            return None;
        }
        if callee.is_none_or(|callee| callee.id() != function) {
            calls.push((call, index));
        }
    }
    (!calls.is_empty()).then_some(calls)
}

/// A call of another step of this project besides a function of this file: a method of this class
/// (`this.m(..)`, maybe inherited), a method of an injected dependency (`this.repo.save(..)`), or
/// a function imported from the project. A global, a package function or a method of a value
/// (`map.get(key)`, `schema.parse(raw)`) is an operation on the value.
fn is_step(file: &File<'_>, facts: &dyn Facts, call: &CallExpression<'_>) -> bool {
    if is_own_import(file, facts, &call.callee) {
        return true;
    }
    let Some(member) = call.callee.get_member_expr() else {
        return false;
    };
    match member.object().get_inner_expression() {
        Expression::ThisExpression(_) => true,
        Expression::StaticMemberExpression(dependency) => {
            matches!(dependency.object, Expression::ThisExpression(_))
                && is_injected(file, call, dependency.property.name.as_str())
        }
        _ => false,
    }
}

/// The name of the callee when the step hands the value, as argument `index`, to another function
/// of this file whose parameter in that place only goes on to steps too: the chain `a -> b -> c`,
/// where `a` and `b` do not read the value.
fn next_step<'a>(
    file: &File<'a>,
    facts: &dyn Facts,
    step: &'a CallExpression<'a>,
    index: usize,
) -> Option<&'a str> {
    // A spread argument shifts the parameters after it.
    if step.arguments[..index]
        .iter()
        .any(|argument| argument.is_spread())
    {
        return None;
    }
    let target = local_callee(file, step)?;
    let BindingPattern::BindingIdentifier(binding) = &params_of(target)?.items.get(index)?.pattern
    else {
        return None;
    };
    steps(file, facts, binding.symbol_id.get()?, target.id())?;
    match step.callee.get_inner_expression() {
        Expression::Identifier(id) => Some(id.name.as_str()),
        callee => callee
            .get_member_expr()
            .and_then(|member| member.static_property_name()),
    }
}

/// The body is one call, also under `return` or `await` (`=> f(..)`): a wrapper only forwards
/// the call.
fn is_wrapper(function: &AstNode<'_>) -> bool {
    let body = match function.kind() {
        AstKind::Function(function) => function.body.as_deref(),
        AstKind::ArrowFunctionExpression(arrow) => match arrow.get_expression() {
            Some(expression) => return is_call(expression),
            None => arrow.get_function_body(),
        },
        _ => None,
    };
    let Some(body) = body else {
        return false;
    };
    match body.statements.as_slice() {
        [Statement::ReturnStatement(statement)] => statement.argument.as_ref().is_some_and(is_call),
        [Statement::ExpressionStatement(statement)] => is_call(&statement.expression),
        _ => false,
    }
}

fn is_call(expression: &Expression<'_>) -> bool {
    let expression = match expression.get_inner_expression() {
        Expression::AwaitExpression(expression) => expression.argument.get_inner_expression(),
        expression => expression,
    };
    match expression {
        Expression::CallExpression(_) | Expression::NewExpression(_) => true,
        Expression::ChainExpression(chain) => {
            matches!(chain.expression, ChainElement::CallExpression(_))
        }
        _ => false,
    }
}

fn params_of<'a>(function: &AstNode<'a>) -> Option<&'a FormalParameters<'a>> {
    match function.kind() {
        AstKind::Function(function) if function.body.is_some() => Some(&function.params),
        AstKind::ArrowFunctionExpression(arrow) => Some(&arrow.params),
        _ => None,
    }
}

/// The constructor of a decorated class (`@Injectable()`): DI fills its parameters.
fn is_di_constructor(file: &File<'_>, function: &AstNode<'_>) -> bool {
    let nodes = file.semantic.nodes();
    let method = nodes.parent_node(function.id());
    matches!(method.kind(), AstKind::MethodDefinition(definition)
        if definition.kind == MethodDefinitionKind::Constructor)
        // method -> `ClassBody` -> `Class`.
        && matches!(
            nodes.parent_kind(nodes.parent_id(method.id())),
            AstKind::Class(class) if !class.decorators.is_empty()
        )
}

/// `name` is a dependency the class receives: a constructor parameter property
/// (`private readonly repo: Repo`) or a decorated field (`@InjectRepository(Order) repo`).
fn is_injected(file: &File<'_>, call: &CallExpression<'_>, name: &str) -> bool {
    let Some(class) = this_class(file, call.node_id.get()) else {
        return false;
    };
    class.body.body.iter().any(|element| match element {
        ClassElement::MethodDefinition(method) if method.kind == MethodDefinitionKind::Constructor => {
            method.value.params.items.iter().any(|parameter| {
                parameter.has_modifier()
                    && matches!(&parameter.pattern, BindingPattern::BindingIdentifier(id) if id.name == name)
            })
        }
        ClassElement::PropertyDefinition(field) => {
            !field.decorators.is_empty() && field.key.static_name().is_some_and(|key| key == name)
        }
        _ => false,
    })
}

/// A function imported by name or by default from this project: a relative path, an alias or a
/// workspace package.
fn is_own_import(file: &File<'_>, facts: &dyn Facts, callee: &Expression<'_>) -> bool {
    let Expression::Identifier(id) = callee.get_inner_expression() else {
        return false;
    };
    let scoping = file.semantic.scoping();
    let nodes = file.semantic.nodes();
    let Some(symbol) = scoping.get_reference(id.reference_id()).symbol_id() else {
        return false;
    };
    let declaration = scoping.symbol_declaration(symbol);
    if !matches!(
        nodes.kind(declaration),
        AstKind::ImportSpecifier(_) | AstKind::ImportDefaultSpecifier(_)
    ) {
        return false;
    }
    matches!(nodes.parent_kind(declaration), AstKind::ImportDeclaration(import)
    if matches!(
        import_kind(file, facts, &import.source.value),
        ImportKind::Relative | ImportKind::Alias | ImportKind::Workspace
    ))
}

/// The function of this file that the call runs: a function declared with a body, a `const`
/// holding a function or an arrow, or `this.m(..)` - a method with a body or a function field of
/// the class whose `this` the call sees. A binding that is written to may hold another function.
fn local_callee<'a>(file: &File<'a>, call: &CallExpression<'a>) -> Option<&'a AstNode<'a>> {
    let nodes = file.semantic.nodes();
    let scoping = file.semantic.scoping();
    match call.callee.get_inner_expression() {
        Expression::Identifier(id) => {
            let symbol = scoping.get_reference(id.reference_id()).symbol_id()?;
            if scoping
                .get_resolved_references(symbol)
                .any(|reference| reference.is_write())
            {
                return None;
            }
            let declaration = scoping.symbol_declaration(symbol);
            match nodes.kind(declaration) {
                AstKind::Function(function) if function.body.is_some() => {
                    Some(nodes.get_node(declaration))
                }
                AstKind::VariableDeclarator(declarator) => {
                    function_value(file, declarator.init.as_ref()?)
                }
                _ => None,
            }
        }
        Expression::StaticMemberExpression(member)
            if matches!(member.object, Expression::ThisExpression(_)) =>
        {
            let name = member.property.name.as_str();
            this_class(file, call.node_id.get())?
                .body
                .body
                .iter()
                .find_map(|element| match element {
                    ClassElement::MethodDefinition(method)
                        if !method.r#static
                            && method.kind == MethodDefinitionKind::Method
                            && method.value.body.is_some()
                            && method.key.static_name().is_some_and(|key| key == name) =>
                    {
                        Some(nodes.get_node(method.value.node_id.get()))
                    }
                    ClassElement::PropertyDefinition(field)
                        if !field.r#static
                            && field.key.static_name().is_some_and(|key| key == name) =>
                    {
                        function_value(file, field.value.as_ref()?)
                    }
                    _ => None,
                })
        }
        _ => None,
    }
}

fn function_value<'a>(file: &File<'a>, value: &Expression<'a>) -> Option<&'a AstNode<'a>> {
    let id = match value.get_inner_expression() {
        Expression::ArrowFunctionExpression(arrow) => arrow.node_id.get(),
        Expression::FunctionExpression(function) => function.node_id.get(),
        _ => return None,
    };
    Some(file.semantic.nodes().get_node(id))
}

/// The class whose instance is `this` at the node: up through arrows to a non-static method or
/// field. Any other function in between has its own `this`, also one inside a field's arrow.
fn this_class<'a>(file: &File<'a>, node: NodeId) -> Option<&'a Class<'a>> {
    let nodes = file.semantic.nodes();
    for ancestor in nodes.ancestors(node) {
        match ancestor.kind() {
            AstKind::Function(_) => {
                let is_member = match nodes.parent_kind(ancestor.id()) {
                    AstKind::MethodDefinition(method) => !method.r#static,
                    AstKind::PropertyDefinition(field) => !field.r#static,
                    _ => false,
                };
                if !is_member {
                    return None;
                }
            }
            AstKind::MethodDefinition(method) if method.r#static => return None,
            AstKind::PropertyDefinition(field) if field.r#static => return None,
            AstKind::StaticBlock(_) => return None,
            AstKind::Class(class) => return Some(class),
            _ => {}
        }
    }
    None
}
