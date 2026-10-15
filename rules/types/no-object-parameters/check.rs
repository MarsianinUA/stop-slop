use kit::oxc::ast::ast::{TSType, TSTypeName, TSTypePredicateName};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::FormalParameter];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let AstKind::FormalParameter(parameter) = node.kind() else {
            return;
        };
        let Some(annotation) = &parameter.type_annotation else {
            return;
        };
        if !is_object(file, &annotation.type_annotation, 0) {
            return;
        }
        let nodes = file.semantic.nodes();
        let function = nodes.parent_node(nodes.parent_id(node.id()));
        if is_callback(function, nodes.parent_kind(function.id())) {
            return;
        }
        if let Some(guarded) = guarded_name(function.kind())
            && parameter.pattern.get_identifier_name().as_deref() == Some(guarded)
        {
            return;
        }
        cx.report(Span::new(parameter.span.start, annotation.span.end));
    }
}

/// `object` itself, a union with it, or the file's own non-generic alias of one
/// (`type Bag = object`), followed up to 8 aliases deep.
fn is_object(file: &File<'_>, ty: &TSType<'_>, depth: u32) -> bool {
    match ty {
        TSType::TSObjectKeyword(_) => true,
        TSType::TSUnionType(union) => union.types.iter().any(|t| is_object(file, t, depth)),
        TSType::TSTypeReference(reference) if depth < 8 && reference.type_arguments.is_none() => {
            let TSTypeName::IdentifierReference(id) = &reference.type_name else {
                return false;
            };
            let scoping = file.semantic.scoping();
            let Some(symbol) = scoping.get_reference(id.reference_id()).symbol_id() else {
                return false;
            };
            let declaration = scoping.symbol_declaration(symbol);
            match file.semantic.nodes().kind(declaration) {
                AstKind::TSTypeAliasDeclaration(alias) if alias.type_parameters.is_none() => {
                    is_object(file, &alias.type_annotation, depth + 1)
                }
                _ => false,
            }
        }
        _ => false,
    }
}

/// A function passed to a call or `new`, returned, or the body of an arrow: the API that
/// calls it sets its parameter types (a decorator, `ValidateIf`, `forEach`).
fn is_callback(function: &AstNode<'_>, parent: AstKind<'_>) -> bool {
    if !matches!(
        function.kind(),
        AstKind::Function(_) | AstKind::ArrowFunctionExpression(_)
    ) {
        return false;
    }
    let span = function.span();
    match parent {
        AstKind::CallExpression(call) => call.arguments.iter().any(|a| a.span() == span),
        AstKind::NewExpression(new) => new.arguments.iter().any(|a| a.span() == span),
        AstKind::ReturnStatement(_) | AstKind::ArrowFunctionExpression(_) => true,
        _ => false,
    }
}

/// The parameter a type guard narrows: `value` in `(value: object): value is Owner`.
fn guarded_name<'a>(function: AstKind<'a>) -> Option<&'a str> {
    let return_type = match function {
        AstKind::Function(f) => f.return_type.as_deref(),
        AstKind::ArrowFunctionExpression(f) => f.return_type.as_deref(),
        AstKind::TSMethodSignature(f) => f.return_type.as_deref(),
        AstKind::TSCallSignatureDeclaration(f) => f.return_type.as_deref(),
        AstKind::TSFunctionType(f) => Some(&*f.return_type),
        _ => None,
    }?;
    let TSType::TSTypePredicate(predicate) = &return_type.type_annotation else {
        return None;
    };
    let TSTypePredicateName::Identifier(name) = &predicate.parameter_name else {
        return None;
    };
    Some(name.name.as_str())
}
