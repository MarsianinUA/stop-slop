use kit::oxc::ast::ast::{Expression, IdentifierReference};
use kit::oxc::semantic::NodeId;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

/// Node modules whose `*Sync` functions block the event loop, with or without `node:`.
const NODE: &[&str] = &["fs", "zlib", "child_process", "crypto"];

/// Packages whose `*Sync` hashing runs on the main thread.
const BCRYPT: &[&str] = &["bcrypt", "bcryptjs"];

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::CallExpression(call) = node.kind() else {
            return;
        };
        let file = cx.file();
        let Some((module, name)) = imported_callee(file, &call.callee) else {
            return;
        };
        let module = module.strip_prefix("node:").unwrap_or(module);
        // Without a callback `randomBytes` fills the buffer before it returns.
        let blocks = name.ends_with("Sync") && (NODE.contains(&module) || BCRYPT.contains(&module))
            || module == "crypto" && name == "randomBytes" && call.arguments.len() < 2;
        if !blocks {
            return;
        }
        let role = match nest_role(file, node.id()) {
            Some(NestRole::RouteHandler) => "a route handler",
            Some(NestRole::MessageHandler) => "a message handler",
            Some(NestRole::ProviderMethod) => "a method that runs while the app serves requests",
            Some(NestRole::Constructor | NestRole::Lifecycle) | None => return,
        };
        let Some(method) = method_name(file, node.id()) else {
            return;
        };
        let callee = one_line(call.callee.span().source_text(file.text));
        cx.report(call.callee.span())
            .arg("call", format!("{callee}(..)"))
            .arg("method", method)
            .arg("role", role);
    }
}

/// The module and the name a callee imports: `("node:fs", "readFileSync")` of `readFileSync`
/// after `import { readFileSync } from 'node:fs'`, `("bcrypt", "hashSync")` of
/// `bcrypt.hashSync` after `import * as bcrypt from 'bcrypt'` or `import bcrypt from 'bcrypt'`.
fn imported_callee<'a>(file: &File<'a>, callee: &'a Expression<'a>) -> Option<(&'a str, &'a str)> {
    let callee = callee.get_inner_expression();
    if let Expression::Identifier(id) = callee {
        return imported_name(file, id);
    }
    let member = callee.get_member_expr()?;
    let Expression::Identifier(object) = member.object().get_inner_expression() else {
        return None;
    };
    Some((whole_module(file, object)?, member.static_property_name()?))
}

/// The module a namespace or default import names.
fn whole_module<'a>(file: &File<'a>, id: &IdentifierReference<'_>) -> Option<&'a str> {
    let scoping = file.semantic.scoping();
    let symbol = scoping.get_reference(id.reference_id()).symbol_id()?;
    let nodes = file.semantic.nodes();
    let declaration = scoping.symbol_declaration(symbol);
    match (nodes.kind(declaration), nodes.parent_kind(declaration)) {
        (
            AstKind::ImportNamespaceSpecifier(_) | AstKind::ImportDefaultSpecifier(_),
            AstKind::ImportDeclaration(import),
        ) => Some(import.source.value.as_str()),
        _ => None,
    }
}

/// `ReportsController.find`: the class and the method around the node.
fn method_name(file: &File<'_>, node: NodeId) -> Option<String> {
    let nodes = file.semantic.nodes();
    nodes.ancestors(node).find_map(|ancestor| {
        let AstKind::MethodDefinition(method) = ancestor.kind() else {
            return None;
        };
        // method -> `ClassBody` -> `Class`.
        let AstKind::Class(class) = nodes.parent_kind(nodes.parent_id(ancestor.id())) else {
            return None;
        };
        let class = class.id.as_ref().map_or("default", |id| id.name.as_str());
        Some(format!(
            "{class}.{}",
            method.key.span().source_text(file.text)
        ))
    })
}
