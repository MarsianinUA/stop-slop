use oxc_ast::AstKind;
use oxc_ast::ast::{TSModuleReference, TSType, TSTypeName};

use crate::{Facts, File, ImportKind, import_kind};

/// A type of this project: a reference whose name the file declares (not a type parameter) or
/// imports from a relative path, a tsconfig alias or a workspace package; also `Own['key']`,
/// `import('./x').T` and a union or intersection with such a member. A name the file neither
/// declares nor imports (`Record`, `Express.Request`) is a lib or global type.
pub fn is_own_type(file: &File<'_>, facts: &dyn Facts, ty: &TSType<'_>) -> bool {
    match ty {
        TSType::TSTypeReference(reference) => is_own_name(file, facts, &reference.type_name),
        TSType::TSIndexedAccessType(access) => is_own_type(file, facts, &access.object_type),
        TSType::TSImportType(import) => is_own_import(file, facts, &import.source.value),
        TSType::TSUnionType(union) => union.types.iter().any(|t| is_own_type(file, facts, t)),
        TSType::TSIntersectionType(intersection) => intersection
            .types
            .iter()
            .any(|t| is_own_type(file, facts, t)),
        _ => false,
    }
}

/// `models.Receipt` is judged by `models`.
fn is_own_name(file: &File<'_>, facts: &dyn Facts, name: &TSTypeName<'_>) -> bool {
    let id = match name {
        TSTypeName::IdentifierReference(id) => id,
        TSTypeName::QualifiedName(qualified) => return is_own_name(file, facts, &qualified.left),
        TSTypeName::ThisExpression(_) => return false,
    };
    let scoping = file.semantic.scoping();
    let Some(symbol) = scoping.get_reference(id.reference_id()).symbol_id() else {
        return false;
    };
    let nodes = file.semantic.nodes();
    let declaration = scoping.symbol_declaration(symbol);
    match nodes.kind(declaration) {
        // A generic parameter, `infer U`, or the key of a mapped type.
        AstKind::TSTypeParameter(_) | AstKind::TSMappedType(_) => false,
        AstKind::ImportSpecifier(_)
        | AstKind::ImportDefaultSpecifier(_)
        | AstKind::ImportNamespaceSpecifier(_) => match nodes.parent_kind(declaration) {
            AstKind::ImportDeclaration(import) => is_own_import(file, facts, &import.source.value),
            _ => false,
        },
        AstKind::TSImportEqualsDeclaration(equals) => match &equals.module_reference {
            TSModuleReference::ExternalModuleReference(reference) => {
                is_own_import(file, facts, &reference.expression.value)
            }
            // `import A = N.B` counts as own even when `N` comes from a package: a rare form.
            _ => true,
        },
        _ => true,
    }
}

fn is_own_import(file: &File<'_>, facts: &dyn Facts, source: &str) -> bool {
    matches!(
        import_kind(file, facts, source),
        ImportKind::Relative | ImportKind::Alias | ImportKind::Workspace
    )
}
