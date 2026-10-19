use kit::oxc::ast::ast::{
    Expression, IdentifierReference, TSSignature, TSType, TSTypeName, UnaryOperator,
};
use kit::oxc::semantic::NodeId;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::UnaryExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::UnaryExpression(unary) = node.kind() else {
            return;
        };
        if unary.operator != UnaryOperator::Delete {
            return;
        }
        let Some(member) = unary.argument.get_inner_expression().get_member_expr() else {
            return;
        };
        // `delete cache[key]` uses the object as a dictionary.
        if member.static_property_name().is_none() {
            return;
        }
        let file = cx.file();
        let Some(origin) = origin(file, node.id(), member.object()) else {
            return;
        };
        cx.report(unary.span)
            .arg(
                "target",
                one_line(unary.argument.span().source_text(file.text)),
            )
            .arg("origin", origin);
    }
}

/// Where an object with a fixed set of keys comes from: `this` of a class, a `const` set to an
/// object literal, or a variable or parameter whose type `is_shape`. Anything else may be a
/// dictionary: `None`.
fn origin(file: &File<'_>, at: NodeId, object: &Expression<'_>) -> Option<String> {
    let id = match object.get_inner_expression() {
        Expression::ThisExpression(_) => {
            let class = this_class(file, at)?;
            return Some(match &class.id {
                Some(id) => format!("`this`, an instance of `{}`", id.name),
                None => "`this`, an instance of a class".to_string(),
            });
        }
        Expression::Identifier(id) => id,
        _ => return None,
    };
    match declared(file, id)? {
        (_, Declared::Const(value)) => match value.get_inner_expression() {
            Expression::ObjectExpression(_) => {
                Some("a variable set to an object literal".to_string())
            }
            _ => None,
        },
        (symbol, Declared::Annotation(ty)) => {
            if !is_shape(file, ty) {
                return None;
            }
            let declaration = file.semantic.scoping().symbol_declaration(symbol);
            let holder = match file.semantic.nodes().kind(declaration) {
                AstKind::VariableDeclarator(_) => "variable",
                _ => "parameter",
            };
            let ty = one_line(ty.span().source_text(file.text));
            Some(format!("a {holder} of type `{ty}`"))
        }
    }
}

/// A type with a fixed set of keys, also with `null` or `undefined` and under `Readonly` or
/// `Partial`: an object type without an index signature, or a type named by a plain name
/// other than lib `Record` and an interface or alias of this file that takes any key. An
/// imported name counts as fixed; `NodeJS.ProcessEnv` and other qualified names do not.
fn is_shape(file: &File<'_>, ty: &TSType<'_>) -> bool {
    is_nullable(ty, |ty| {
        let ty = unwrapped_type(file, ty);
        match ty {
            TSType::TSTypeLiteral(literal) => !has_index(&literal.members),
            TSType::TSTypeReference(reference) => match &reference.type_name {
                TSTypeName::IdentifierReference(id) => {
                    lib_type(file, ty, "Record").is_none() && !is_own_dictionary(file, id)
                }
                _ => false,
            },
            _ => false,
        }
    })
}

fn is_own_dictionary(file: &File<'_>, id: &IdentifierReference<'_>) -> bool {
    let scoping = file.semantic.scoping();
    let Some(symbol) = scoping.get_reference(id.reference_id()).symbol_id() else {
        return false;
    };
    match file
        .semantic
        .nodes()
        .kind(scoping.symbol_declaration(symbol))
    {
        AstKind::TSInterfaceDeclaration(interface) => has_index(&interface.body.body),
        AstKind::TSTypeAliasDeclaration(alias) => {
            match unwrapped_type(file, &alias.type_annotation) {
                TSType::TSTypeLiteral(literal) => has_index(&literal.members),
                TSType::TSMappedType(_) => true,
                ty => lib_type(file, ty, "Record").is_some(),
            }
        }
        _ => false,
    }
}

fn has_index(members: &[TSSignature<'_>]) -> bool {
    members
        .iter()
        .any(|member| matches!(member, TSSignature::TSIndexSignature(_)))
}
