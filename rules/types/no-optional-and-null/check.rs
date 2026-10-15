use kit::oxc::ast::ast::TSType;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[
        AstType::TSPropertySignature,
        AstType::PropertyDefinition,
        AstType::FormalParameter,
    ];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let (name, annotation) = match node.kind() {
            AstKind::TSPropertySignature(field) if field.optional => {
                (field.key.span(), &field.type_annotation)
            }
            AstKind::PropertyDefinition(field) if field.optional => {
                (field.key.span(), &field.type_annotation)
            }
            // `constructor(public note?: string | null)` declares a field.
            AstKind::FormalParameter(parameter)
                if parameter.optional && parameter.has_modifier() =>
            {
                (parameter.pattern.span(), &parameter.type_annotation)
            }
            _ => return,
        };
        let Some(annotation) = annotation else {
            return;
        };
        if has_null(&annotation.type_annotation) {
            cx.report(Span::new(name.start, annotation.span.end));
        }
    }
}

fn has_null(ty: &TSType<'_>) -> bool {
    match ty {
        TSType::TSNullKeyword(_) => true,
        TSType::TSUnionType(union) => union.types.iter().any(has_null),
        _ => false,
    }
}
