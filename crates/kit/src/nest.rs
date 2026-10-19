use oxc_ast::AstKind;
use oxc_ast::ast::{Decorator, Expression, MethodDefinitionKind};
use oxc_semantic::NodeId;
use oxc_span::GetSpan;

use crate::File;
use crate::callback::member_class;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NestRole {
    /// `@Get()` and the other HTTP methods, or `@Query()`, `@Mutation()` and `@ResolveField()`
    /// of a GraphQL resolver.
    RouteHandler,
    /// `@MessagePattern()`, `@EventPattern()` or a gateway's `@SubscribeMessage()`.
    MessageHandler,
    /// A hook Nest calls once, at startup or shutdown: `onModuleInit`, `afterInit`...
    Lifecycle,
    Constructor,
    /// Any other method: it runs while the app serves, like a provider's.
    ProviderMethod,
}

/// The classes Nest makes and calls: controllers, GraphQL resolvers, WebSocket gateways and
/// providers.
const CLASSES: &[&str] = &["Controller", "Resolver", "WebSocketGateway", "Injectable"];

const ROUTES: &[&str] = &[
    "Get",
    "Post",
    "Put",
    "Patch",
    "Delete",
    "Options",
    "Head",
    "All",
    "Search",
    "Sse",
    "Query",
    "Mutation",
    "Subscription",
    "ResolveField",
];

const MESSAGES: &[&str] = &["MessagePattern", "EventPattern", "SubscribeMessage"];

/// The methods of `OnModuleInit`, `OnApplicationBootstrap`, `OnModuleDestroy`,
/// `BeforeApplicationShutdown` and `OnApplicationShutdown`.
const LIFECYCLE: &[&str] = &[
    "onModuleInit",
    "onApplicationBootstrap",
    "onModuleDestroy",
    "beforeApplicationShutdown",
    "onApplicationShutdown",
];

/// The role of the class method around `node`; `None` outside a class NestJS decorates. The
/// node counts only inside the method's function, and the innermost class decides: a class
/// declared inside a method has its own role. Decorators match by the name they call, as
/// `@Controller()` reads, wherever it is imported from.
pub fn nest_role(file: &File<'_>, node: NodeId) -> Option<NestRole> {
    let nodes = file.semantic.nodes();
    let span = nodes.get_node(node).span();
    let member = nodes.ancestors(node).find(|ancestor| {
        matches!(
            ancestor.kind(),
            AstKind::MethodDefinition(_) | AstKind::Class(_)
        )
    })?;
    let AstKind::MethodDefinition(method) = member.kind() else {
        return None;
    };
    if !method.value.span.contains_inclusive(span) {
        return None;
    }
    let class = member_class(file, member)?;
    if !decorated(&class.decorators, CLASSES) {
        return None;
    }
    let is_hook = |name: &str| {
        !method.r#static
            && method.kind == MethodDefinitionKind::Method
            && method.key.is_specific_static_name(name)
    };
    let role = if method.kind == MethodDefinitionKind::Constructor {
        NestRole::Constructor
    } else if decorated(&method.decorators, ROUTES) {
        NestRole::RouteHandler
    } else if decorated(&method.decorators, MESSAGES) {
        NestRole::MessageHandler
    } else if LIFECYCLE.iter().any(|hook| is_hook(hook))
        // `OnGatewayInit`: only a gateway has it.
        || is_hook("afterInit") && decorated(&class.decorators, &["WebSocketGateway"])
    {
        NestRole::Lifecycle
    } else {
        NestRole::ProviderMethod
    };
    Some(role)
}

fn decorated(decorators: &[Decorator<'_>], names: &[&str]) -> bool {
    decorators
        .iter()
        .filter_map(decorator_name)
        .any(|name| names.contains(&name))
}

/// `Controller` of `@Controller('orders')` and of `@Controller`.
fn decorator_name<'a>(decorator: &Decorator<'a>) -> Option<&'a str> {
    let callee = match &decorator.expression {
        Expression::CallExpression(call) => &call.callee,
        expression => expression,
    };
    match callee {
        Expression::Identifier(id) => Some(id.name.as_str()),
        _ => None,
    }
}
