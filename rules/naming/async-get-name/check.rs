use kit::oxc::ast::ast::{
    Class, Expression, ForOfStatement, Function, MethodDefinitionKind, TSTypeAnnotation,
};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::MethodDefinition];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let AstKind::MethodDefinition(method) = node.kind() else {
            return;
        };
        if method.kind != MethodDefinitionKind::Method || is_controller_file(file) {
            return;
        }
        let Some(name) = method.key.name() else {
            return;
        };
        let nodes = file.semantic.nodes();
        // The method's parent is the class body.
        let in_controller = matches!(
            nodes.parent_kind(nodes.parent_id(node.id())),
            AstKind::Class(class) if is_controller(class)
        );
        if is_get_name(&name) && !in_controller && function_awaits(file, &method.value) {
            cx.report(method.key.span()).arg("name", name);
        }
    }

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        if is_controller_file(file) {
            return;
        }
        let scoping = file.semantic.scoping();
        let nodes = file.semantic.nodes();
        for symbol in scoping.symbol_ids() {
            let name = scoping.symbol_name(symbol);
            if !is_get_name(name) || !is_own_name(file, symbol) {
                continue;
            }
            // Each overload declares the symbol, and only the last one has a body. The own name
            // of a function expression repeats the variable it initializes.
            let awaits = scoping.symbol_declarations(symbol).any(|declaration| {
                match nodes.kind(declaration) {
                    AstKind::Function(function) if function.is_declaration() => {
                        function_awaits(file, function)
                    }
                    AstKind::VariableDeclarator(declarator)
                        if declarator.id.is_binding_identifier() =>
                    {
                        match declarator
                            .init
                            .as_ref()
                            .map(Expression::get_inner_expression)
                        {
                            Some(Expression::FunctionExpression(function)) => {
                                function_awaits(file, function)
                            }
                            Some(Expression::ArrowFunctionExpression(arrow)) => awaits(
                                file,
                                arrow.r#async,
                                arrow.return_type.as_deref(),
                                arrow.body.span(),
                            ),
                            _ => false,
                        }
                    }
                    _ => false,
                }
            });
            if awaits {
                cx.report(scoping.symbol_span(symbol)).arg("name", name);
            }
        }
    }
}

/// `getUser`, not `get` (a key-value protocol: `cache.get(key)`) or `getOrSet`, `getOrCreate`
/// (a cache protocol).
fn is_get_name(name: &str) -> bool {
    matches!(words(name).as_slice(), [get, second, ..]
        if get.eq_ignore_ascii_case("get") && !second.eq_ignore_ascii_case("or"))
}

/// A handler's `get*` names an HTTP GET, not a fetch.
fn is_controller_file(file: &File<'_>) -> bool {
    kit::suffix_of(file.path) == Some("controller")
}

fn is_controller(class: &Class<'_>) -> bool {
    class.decorators.iter().any(|decorator| {
        let callee = match &decorator.expression {
            Expression::CallExpression(call) => &call.callee,
            other => other,
        };
        matches!(callee, Expression::Identifier(id) if id.name == "Controller")
    })
}

fn function_awaits(file: &File<'_>, function: &Function<'_>) -> bool {
    function.body.as_ref().is_some_and(|body| {
        awaits(
            file,
            function.r#async,
            function.return_type.as_deref(),
            body.span,
        )
    })
}

/// Async (or returns a lib `Promise`) and awaits in its body, nested functions included: without
/// an `await` a `get*` is usually a lazy getter (`getHandle()`), with one it waits on someone
/// else's work.
fn awaits(
    file: &File<'_>,
    is_async: bool,
    return_type: Option<&TSTypeAnnotation<'_>>,
    body: Span,
) -> bool {
    let returns_promise =
        return_type.is_some_and(|ty| lib_type(file, &ty.type_annotation, "Promise").is_some());
    (is_async || returns_promise)
        && file.semantic.nodes().iter().any(|node| {
            matches!(
                node.kind(),
                AstKind::AwaitExpression(_)
                    | AstKind::ForOfStatement(ForOfStatement { r#await: true, .. })
            ) && body.contains_inclusive(node.span())
        })
}
