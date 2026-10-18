use kit::oxc::ast::ast::{
    BindingPattern, Expression, TSType, UnaryOperator, VariableDeclarationKind,
};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::VariableDeclarator];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let AstKind::VariableDeclarator(declarator) = node.kind() else {
            return;
        };
        let (BindingPattern::BindingIdentifier(id), Some(annotation), Some(init)) = (
            &declarator.id,
            &declarator.type_annotation,
            &declarator.init,
        ) else {
            return;
        };
        let ty = &annotation.type_annotation;
        let Some(literal) = repeated_type(file, ty, init) else {
            return;
        };
        let constant = matches!(
            file.semantic.nodes().parent_kind(node.id()),
            AstKind::VariableDeclaration(declaration)
                if declaration.kind == VariableDeclarationKind::Const
        );
        // Without the annotation a `const` holding a literal gets the literal type (`5`, not
        // `number`).
        let narrowing_noticed = literal
            && constant
            && id
                .symbol_id
                .get()
                .is_some_and(|symbol| relies_on_wide_type(file, symbol));
        let reported = cx
            .report(Span::new(id.span.start, annotation.span.end))
            .arg("type", ty.span().source_text(file.text));
        if !narrowing_noticed {
            reported.fix([Edit {
                span: annotation.span,
                text: String::new(),
            }]);
        }
    }
}

/// `Some` when the value gives the annotated type by itself; `Some(true)` when the value is a
/// literal that a `const` would keep as its own type.
fn repeated_type(file: &File<'_>, ty: &TSType<'_>, init: &Expression<'_>) -> Option<bool> {
    let unsigned = match init {
        Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::UnaryNegation => {
            &unary.argument
        }
        _ => init,
    };
    let primitive = match ty {
        TSType::TSNumberKeyword(_) => matches!(unsigned, Expression::NumericLiteral(_)),
        TSType::TSBigIntKeyword(_) => matches!(unsigned, Expression::BigIntLiteral(_)),
        TSType::TSStringKeyword(_) => matches!(
            init,
            Expression::StringLiteral(_) | Expression::TemplateLiteral(_)
        ),
        TSType::TSBooleanKeyword(_) => matches!(init, Expression::BooleanLiteral(_)),
        _ => false,
    };
    if primitive {
        return Some(narrows_to_literal(init));
    }
    let same = match init {
        Expression::RegExpLiteral(_) => {
            lib_type(file, ty, "RegExp").is_some_and(|regexp| regexp.type_arguments.is_none())
        }
        // `Map<string, Order> = new Map()` differs: without the annotation it is `Map<any, any>`.
        Expression::NewExpression(new) if matches!(ty, TSType::TSTypeReference(_)) => {
            let class_end = new
                .type_arguments
                .as_ref()
                .map_or(new.callee.span().end, |arguments| arguments.span.end);
            let class = Span::new(new.callee.span().start, class_end);
            same_without_spaces(
                ty.span().source_text(file.text),
                class.source_text(file.text),
            )
        }
        _ => false,
    };
    same.then_some(false)
}

fn same_without_spaces(a: &str, b: &str) -> bool {
    let solid = |c: &char| !c.is_whitespace();
    a.chars().filter(solid).eq(b.chars().filter(solid))
}
