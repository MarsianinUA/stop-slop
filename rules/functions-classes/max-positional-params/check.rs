use kit::oxc::ast::ast::{BindingPattern, FormalParameters, MethodDefinitionKind};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    max: u32,
}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::Function, AstType::ArrowFunctionExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let params = match node.kind() {
            // Without a body it is a signature: an overload, an `abstract` method, a `declare`.
            AstKind::Function(function) if function.body.is_some() => &function.params,
            AstKind::ArrowFunctionExpression(arrow) => &arrow.params,
            _ => return,
        };
        // A decorated parameter is filled by the framework; a destructured object is the options.
        let count = params
            .items
            .iter()
            .filter(|parameter| {
                parameter.decorators.is_empty()
                    && !matches!(parameter.pattern, BindingPattern::ObjectPattern(_))
            })
            .count()
            + usize::from(params.rest.is_some());
        if count <= self.max as usize
            || is_ambient(file, node.id())
            || is_signature_given(file, node)
            || is_injected(file, node, params)
        {
            return;
        }
        cx.report(params.span)
            .arg("count", count)
            .arg("max", self.max);
    }
}

/// A constructor that DI fills: the class has a decorator (`@Injectable()`), or a parameter is a
/// property (`private readonly store: Store`) or has a decorator (`@Inject(TOKEN)`).
fn is_injected(file: &File<'_>, function: &AstNode<'_>, params: &FormalParameters<'_>) -> bool {
    let nodes = file.semantic.nodes();
    let method = nodes.parent_node(function.id());
    matches!(method.kind(), AstKind::MethodDefinition(definition)
        if definition.kind == MethodDefinitionKind::Constructor)
        && (params
            .items
            .iter()
            .any(|parameter| parameter.has_modifier() || !parameter.decorators.is_empty())
            // method -> `ClassBody` -> `Class`.
            || matches!(
                nodes.parent_kind(nodes.parent_id(method.id())),
                AstKind::Class(class) if !class.decorators.is_empty()
            ))
}
