use kit::oxc::ast::ast::{BindingPattern, ImportDeclarationSpecifier, Statement};
use kit::oxc::semantic::SymbolId;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// A word of the file name that makes the file an env helper: `env.ts`, `require-env.ts`.
    helpers: Vec<String>,
}

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        if kit::suffix_of(file.path) == Some("config") || file_name_has_word(file, &self.helpers) {
            return;
        }
        let scoping = file.semantic.scoping();
        let nodes = file.semantic.nodes();
        let (processes, env_imports) = imports(file);
        let imported = processes.into_iter().flat_map(|symbol| {
            scoping
                .get_resolved_references(symbol)
                .map(|reference| nodes.get_node(reference.node_id()))
        });
        let reads = global_references(file, "process")
            .chain(imported)
            .filter_map(|node| env_read(file, node));
        for span in env_imports.into_iter().chain(reads) {
            cx.report(span)
                .arg("access", one_line(span.source_text(file.text)));
        }
    }
}

/// From Node's `process` module: the symbols of `process` itself (`import process from`,
/// `import * as process from`) and the spans of `env` imported by name (`import { env } from`),
/// a read of the environment on its own.
fn imports(file: &File<'_>) -> (Vec<SymbolId>, Vec<Span>) {
    let mut processes = Vec::new();
    let mut reads = Vec::new();
    for statement in &file.semantic.nodes().program().body {
        let Statement::ImportDeclaration(import) = statement else {
            continue;
        };
        if !import.import_kind.is_value()
            || !matches!(import.source.value.as_str(), "process" | "node:process")
        {
            continue;
        }
        for specifier in import.specifiers.iter().flatten() {
            let local = match specifier {
                ImportDeclarationSpecifier::ImportDefaultSpecifier(default) => &default.local,
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(namespace) => &namespace.local,
                ImportDeclarationSpecifier::ImportSpecifier(named)
                    if !named.import_kind.is_value() =>
                {
                    continue;
                }
                ImportDeclarationSpecifier::ImportSpecifier(named)
                    if named.imported.name() == "default" =>
                {
                    &named.local
                }
                ImportDeclarationSpecifier::ImportSpecifier(named)
                    if named.imported.name() == "env" =>
                {
                    reads.push(named.span);
                    continue;
                }
                ImportDeclarationSpecifier::ImportSpecifier(_) => continue,
            };
            processes.extend(local.symbol_id.get());
        }
    }
    (processes, reads)
}

/// The read of the environment when the reference to `process` gives it: `process.env` with
/// what it is read for (`process.env.PORT`, `process.env[key]`, `const { PORT } = process.env`),
/// or a destructuring that takes `env` (`const { env } = process`).
fn env_read<'a>(file: &File<'a>, reference: &AstNode<'a>) -> Option<Span> {
    let (_, parent) = use_of(file, reference);
    if let AstKind::VariableDeclarator(declarator) = parent.kind() {
        let BindingPattern::ObjectPattern(pattern) = &declarator.id else {
            return None;
        };
        return pattern
            .properties
            .iter()
            .any(|property| property.key.static_name().is_some_and(|key| key == "env"))
            .then_some(declarator.span);
    }
    if !matches!(member_use(file, parent), Some(MemberUse::Name(name)) if name == "env") {
        return None;
    }
    let (env, parent) = use_of(file, parent);
    Some(match parent.kind() {
        AstKind::VariableDeclarator(declarator)
            if matches!(declarator.id, BindingPattern::ObjectPattern(_)) =>
        {
            declarator.span
        }
        kind if kind
            .as_member_expression_kind()
            .is_some_and(|member| member.object().span() == env.span()) =>
        {
            parent.span()
        }
        _ => env.span(),
    })
}

/// The node with the assertions around it, and the node that uses it.
fn use_of<'a>(file: &File<'a>, node: &AstNode<'a>) -> (&'a AstNode<'a>, &'a AstNode<'a>) {
    let node = outer_assertion(file, node);
    (node, file.semantic.nodes().parent_node(node.id()))
}
