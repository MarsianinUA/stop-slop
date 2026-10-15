use oxc_ast::AstKind;
use oxc_semantic::{AstNode, AstNodes, NodeId, SymbolId};
use oxc_span::GetSpan;

use crate::File;

/// The symbol leaves the file: a declaration of it (a merged name has several) is under
/// `export` or `export default` (at the top of the file or in a namespace), or it is named by
/// `export { x }`, `export { x as y }`, `export type { X }`, `export default x` or `export = x`.
pub fn is_exported(file: &File<'_>, symbol: SymbolId) -> bool {
    let nodes = file.semantic.nodes();
    file.semantic
        .scoping()
        .symbol_declarations(symbol)
        .any(|declaration| {
            // A declarator sits in `VariableDeclaration`; `export` wraps that statement.
            let statement = match nodes.kind(declaration) {
                AstKind::VariableDeclarator(_) => nodes.parent_id(declaration),
                _ => declaration,
            };
            is_under_export(nodes, statement)
        })
        || is_named_by_export(file, symbol)
}

/// A function, class, interface, type alias or enum that leaves the file: it is under `export`
/// or `export default`, its name is exported by name, or it initializes an exported variable
/// (`export const f = () => {}`, `export const C = class {}`). Of a merged name only this
/// declaration counts: `export const Kind = ..; type Kind = ..` keeps the type local.
pub fn is_exported_node(file: &File<'_>, node: &AstNode<'_>) -> bool {
    let nodes = file.semantic.nodes();
    let own_name = match node.kind() {
        AstKind::Function(function) => function.id.as_ref(),
        AstKind::Class(class) => class.id.as_ref(),
        AstKind::TSInterfaceDeclaration(interface) => Some(&interface.id),
        AstKind::TSTypeAliasDeclaration(alias) => Some(&alias.id),
        AstKind::TSEnumDeclaration(declaration) => Some(&declaration.id),
        _ => None,
    };
    if is_under_export(nodes, node.id())
        || own_name
            .and_then(|id| id.symbol_id.get())
            .is_some_and(|symbol| is_named_by_export(file, symbol))
    {
        return true;
    }
    match nodes.parent_kind(node.id()) {
        AstKind::VariableDeclarator(declarator) => {
            declarator
                .init
                .as_ref()
                .is_some_and(|init| init.span() == node.span())
                && declarator
                    .id
                    .get_binding_identifier()
                    .and_then(|id| id.symbol_id.get())
                    .is_some_and(|symbol| is_exported(file, symbol))
        }
        _ => false,
    }
}

fn is_under_export(nodes: &AstNodes<'_>, statement: NodeId) -> bool {
    matches!(
        nodes.parent_kind(statement),
        AstKind::ExportDeclaration(_) | AstKind::ExportDefaultDeclaration(_)
    )
}

fn is_named_by_export(file: &File<'_>, symbol: SymbolId) -> bool {
    let nodes = file.semantic.nodes();
    file.semantic
        .scoping()
        .get_resolved_references(symbol)
        .any(|reference| {
            matches!(
                nodes.parent_kind(reference.node_id()),
                AstKind::ExportSpecifier(_)
                    | AstKind::ExportDefaultDeclaration(_)
                    | AstKind::TSExportAssignment(_)
            )
        })
}
