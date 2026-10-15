use oxc_ast::AstKind;
use oxc_ast::ast::BindingIdentifier;
use oxc_semantic::{AstNode, SymbolId};
use oxc_span::GetSpan;

use crate::File;

/// The symbol leaves the file: declared under `export` or `export default` (at the top of the
/// file or in a namespace), or named by `export { x }`, `export { x as y }`, `export type { X }`,
/// `export default x` or `export = x`.
pub fn is_exported(file: &File<'_>, symbol: SymbolId) -> bool {
    let nodes = file.semantic.nodes();
    let scoping = file.semantic.scoping();
    let declaration = scoping.symbol_declaration(symbol);
    // A declarator sits in `VariableDeclaration`; `export` wraps that statement.
    let statement = match nodes.kind(declaration) {
        AstKind::VariableDeclarator(_) => nodes.parent_id(declaration),
        _ => declaration,
    };
    matches!(
        nodes.parent_kind(statement),
        AstKind::ExportDeclaration(_) | AstKind::ExportDefaultDeclaration(_)
    ) || scoping.get_resolved_references(symbol).any(|reference| {
        matches!(
            nodes.parent_kind(reference.node_id()),
            AstKind::ExportSpecifier(_)
                | AstKind::ExportDefaultDeclaration(_)
                | AstKind::TSExportAssignment(_)
        )
    })
}

/// A function, class, interface, type alias or enum that leaves the file: its own name is
/// exported, it is the default export, or it initializes an exported variable
/// (`export const f = () => {}`, `export const C = class {}`).
pub fn is_exported_node(file: &File<'_>, node: &AstNode<'_>) -> bool {
    let own_name = match node.kind() {
        AstKind::Function(function) => function.id.as_ref(),
        AstKind::Class(class) => class.id.as_ref(),
        AstKind::TSInterfaceDeclaration(interface) => Some(&interface.id),
        AstKind::TSTypeAliasDeclaration(alias) => Some(&alias.id),
        AstKind::TSEnumDeclaration(declaration) => Some(&declaration.id),
        _ => None,
    };
    let exported_binding = |id: &BindingIdentifier<'_>| {
        id.symbol_id
            .get()
            .is_some_and(|symbol| is_exported(file, symbol))
    };
    own_name.is_some_and(exported_binding)
        || match file.semantic.nodes().parent_kind(node.id()) {
            AstKind::ExportDefaultDeclaration(_) => true,
            AstKind::VariableDeclarator(declarator) => {
                declarator
                    .init
                    .as_ref()
                    .is_some_and(|init| init.span() == node.span())
                    && declarator
                        .id
                        .get_binding_identifier()
                        .is_some_and(exported_binding)
            }
            _ => false,
        }
}
