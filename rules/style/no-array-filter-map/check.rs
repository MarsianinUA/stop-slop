use kit::oxc::ast::ast::{CallExpression, Expression};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::CallExpression(call) = node.kind() else {
            return;
        };
        let Some((second, receiver)) = method_call(call) else {
            return;
        };
        let first = match second {
            "map" => "filter",
            "filter" => "map",
            _ => return,
        };
        let Some((name, array)) = call_of(receiver).and_then(method_call) else {
            return;
        };
        let file = cx.file();
        // `rows.filter(..).map(..)` of a query builder or an iterator is not two array passes.
        if name == first && is_known_array(file, array) {
            cx.report(call.span)
                .arg("first", first)
                .arg("second", second);
        }
    }
}

/// `x.name(..)` and `x['name'](..)`, also through `?.`: the method's name and `x`.
fn method_call<'b, 'a>(call: &'b CallExpression<'a>) -> Option<(&'a str, &'b Expression<'a>)> {
    let member = call.callee.get_member_expr()?;
    Some((member.static_property_name()?, member.object()))
}
