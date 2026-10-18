use kit::oxc::ast::ast::{
    BindingIdentifier, Declaration, ExportDefaultDeclarationKind, IdentifierReference, Statement,
};
use kit::oxc::semantic::NodeId;
use kit::prelude::*;
use kit::suffix_of;

/// File suffixes where a type may sit next to the code it is derived from: `orders.config.ts`,
/// `order.schema.ts`.
#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    suffixes: Vec<String>,
}

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let body = &file.semantic.nodes().program().body;
        // A file of types alone is a types file whatever its name.
        if file.role == Some(Role::Types)
            || suffix_of(file.path).is_some_and(|suffix| self.suffixes.iter().any(|s| s == suffix))
            || !body.iter().any(|statement| is_code(file, statement))
        {
            return;
        }
        // In a constants file `no-type-in-constants-file` reports types and interfaces.
        let enums_only = file.role == Some(Role::Constants);
        for statement in body {
            let Some(declaration) = type_declaration(statement) else {
                continue;
            };
            if enums_only && !declaration.is_enum {
                continue;
            }
            let id = declaration.id;
            if is_exported_node(file, file.semantic.nodes().get_node(declaration.node)) {
                cx.report(Span::new(declaration.start, id.span.end))
                    .arg("name", id.name.as_str());
            }
        }
    }
}

/// A statement that runs when the module loads. Imports, exports by name, types and everything
/// `declare` (`declare module 'm'`, `declare global` too) only describe.
fn is_code(file: &File<'_>, statement: &Statement<'_>) -> bool {
    match statement {
        Statement::ExportDeclaration(export) => is_code_declaration(&export.declaration),
        Statement::ExportDefaultDeclaration(export) => match &export.declaration {
            ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => false,
            ExportDefaultDeclarationKind::Identifier(id) => names_value(file, id),
            _ => true,
        },
        Statement::ImportDeclaration(_)
        | Statement::ExportNamedDeclaration(_)
        | Statement::ExportFromDeclaration(_)
        | Statement::ExportAllDeclaration(_)
        | Statement::TSExportAssignment(_)
        | Statement::TSNamespaceExportDeclaration(_)
        | Statement::EmptyStatement(_) => false,
        _ => statement.as_declaration().is_none_or(is_code_declaration),
    }
}

/// `export default Order` of an interface or a type alias emits nothing; an unresolved name is
/// a global value.
fn names_value(file: &File<'_>, id: &IdentifierReference<'_>) -> bool {
    let scoping = file.semantic.scoping();
    scoping
        .get_reference(id.reference_id())
        .symbol_id()
        .is_none_or(|symbol| scoping.symbol_flags(symbol).is_value())
}

fn is_code_declaration(declaration: &Declaration<'_>) -> bool {
    match declaration {
        Declaration::VariableDeclaration(variable) => !variable.declare,
        Declaration::FunctionDeclaration(function) => !function.declare,
        Declaration::ClassDeclaration(class) => !class.declare,
        Declaration::TSNamespaceDeclaration(namespace) => !namespace.declare,
        _ => false,
    }
}

/// A type alias, interface or enum, alone or under `export` / `export default`.
struct TypeDeclaration<'s, 'a> {
    /// Where the head starts: `type`, `interface`, `enum`, `const`, `declare`.
    start: u32,
    id: &'s BindingIdentifier<'a>,
    node: NodeId,
    is_enum: bool,
}

fn type_declaration<'s, 'a>(statement: &'s Statement<'a>) -> Option<TypeDeclaration<'s, 'a>> {
    let declaration = match statement {
        Statement::ExportDeclaration(export) => &export.declaration,
        Statement::ExportDefaultDeclaration(export) => {
            return match &export.declaration {
                ExportDefaultDeclarationKind::TSInterfaceDeclaration(interface) => {
                    Some(TypeDeclaration {
                        start: interface.span.start,
                        id: &interface.id,
                        node: interface.node_id.get(),
                        is_enum: false,
                    })
                }
                _ => None,
            };
        }
        _ => statement.as_declaration()?,
    };
    let (start, id, node, is_enum) = match declaration {
        Declaration::TSTypeAliasDeclaration(alias) => {
            (alias.span.start, &alias.id, alias.node_id.get(), false)
        }
        Declaration::TSInterfaceDeclaration(interface) => (
            interface.span.start,
            &interface.id,
            interface.node_id.get(),
            false,
        ),
        Declaration::TSEnumDeclaration(declaration) => (
            declaration.span.start,
            &declaration.id,
            declaration.node_id.get(),
            true,
        ),
        _ => return None,
    };
    Some(TypeDeclaration {
        start,
        id,
        node,
        is_enum,
    })
}
