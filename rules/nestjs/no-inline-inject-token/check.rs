use kit::oxc::ast::ast::{Argument, Expression};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::Decorator];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::Decorator(decorator) = node.kind() else {
            return;
        };
        let Expression::CallExpression(call) = &decorator.expression else {
            return;
        };
        let Expression::Identifier(callee) = &call.callee else {
            return;
        };
        let is_string = match call.arguments.first() {
            Some(Argument::StringLiteral(_)) => true,
            Some(Argument::TemplateLiteral(template)) => template.is_no_substitution_template(),
            _ => false,
        };
        if !is_string {
            return;
        }
        let file = cx.file();
        if matches!(imported_name(file, callee),
            Some((module, "Inject")) if package_name(module) == "@nestjs/common")
        {
            cx.report(decorator.span)
                .arg("decorator", one_line(decorator.span.source_text(file.text)));
        }
    }
}
