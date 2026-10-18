use kit::oxc::ast::ast::{Class, Expression, IdentifierReference};
use kit::oxc::semantic::SymbolFlags;
use kit::prelude::*;
use kit::suffix_of;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    owners: Vec<String>,
    clients: Vec<String>,
}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::NewExpression, AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let name = match node.kind() {
            AstKind::NewExpression(new) => match new.callee.get_inner_expression() {
                Expression::Identifier(id) => Some(id.name.as_str()),
                callee => callee
                    .get_member_expr()
                    .and_then(|member| member.static_property_name()),
            },
            // An imported factory, by name or through its namespace (`redis.createClient(..)`): a
            // method (`this.repo.createClient(..)`) or a function of this file is not a client
            // library's.
            AstKind::CallExpression(call) => {
                let (id, name) = match call.callee.get_inner_expression() {
                    Expression::Identifier(id) => (id, id.name.as_str()),
                    Expression::StaticMemberExpression(member) => {
                        match member.object.get_inner_expression() {
                            Expression::Identifier(id) => (id, member.property.name.as_str()),
                            _ => return,
                        }
                    }
                    _ => return,
                };
                (is_import(file, id) && words(name).first() == Some(&"create")).then_some(name)
            }
            _ => None,
        };
        let Some(name) = name.filter(|name| ends_with_word(&self.clients, name)) else {
            return;
        };
        let Some(class) = owner_class(file, node) else {
            return;
        };
        let class_name = class.id.as_ref().map(|id| id.name.as_str());
        let is_owner = suffix_of(file.path).is_some_and(|suffix| {
            self.owners
                .iter()
                .any(|owner| owner.eq_ignore_ascii_case(suffix))
        }) || class_name
            .is_some_and(|class_name| ends_with_word(&self.owners, class_name));
        if is_owner {
            cx.report(node.span())
                .arg("name", name)
                .arg("class", class_name.unwrap_or("default"));
        }
    }
}

fn is_import(file: &File<'_>, id: &IdentifierReference<'_>) -> bool {
    let scoping = file.semantic.scoping();
    scoping
        .get_reference(id.reference_id())
        .symbol_id()
        .is_some_and(|symbol| scoping.symbol_flags(symbol).contains(SymbolFlags::Import))
}

fn ends_with_word(list: &[String], name: &str) -> bool {
    words(name)
        .last()
        .is_some_and(|word| list.iter().any(|item| item.eq_ignore_ascii_case(word)))
}

/// The class whose non-static member holds the node. A static member is a factory; a decorator,
/// of the class or of a member, runs once when the class is defined; code outside a member (a
/// function of the file) is not the class's.
fn owner_class<'a>(file: &File<'a>, node: &AstNode<'a>) -> Option<&'a Class<'a>> {
    let mut ancestors = file.semantic.nodes().ancestor_kinds(node.id());
    let per_instance = ancestors.find_map(|kind| match kind {
        AstKind::MethodDefinition(member) => Some(!member.r#static),
        AstKind::PropertyDefinition(member) => Some(!member.r#static),
        AstKind::AccessorProperty(member) => Some(!member.r#static),
        AstKind::StaticBlock(_) | AstKind::Decorator(_) => Some(false),
        _ => None,
    })?;
    if !per_instance {
        return None;
    }
    ancestors.find_map(|kind| match kind {
        AstKind::Class(class) => Some(class),
        _ => None,
    })
}
