use oxc_ast::AstKind;
use oxc_ast::ast::{Argument, Expression, IdentifierReference, StringLiteral};

use crate::{Facts, File};

/// Where an import specifier points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportKind {
    /// `node:fs`, or a bare builtin like `fs` that the file's packages do not declare.
    Node,
    External,
    /// A `workspace:` package of the same monorepo, with or without a subpath.
    Workspace,
    /// A key of `paths` in the nearest `tsconfig.json`, like `@/*`, or a subpath import of
    /// `package.json` (`#db/client`).
    Alias,
    /// `./x`, `../x`, `.`, `..`, `/x`.
    Relative,
}

impl ImportKind {
    /// A module of this project: a relative path, an alias or a workspace package.
    pub fn is_own(self) -> bool {
        matches!(
            self,
            ImportKind::Relative | ImportKind::Alias | ImportKind::Workspace
        )
    }
}

// `require('module').builtinModules` of Node 24.13.1, without the four that exist only with
// `node:` (`node:test`, `node:sqlite`, `node:sea`, `node:test/reporters`): a bare `test` is an
// npm package.
const BUILTINS: &[&str] = &[
    "_http_agent",
    "_http_client",
    "_http_common",
    "_http_incoming",
    "_http_outgoing",
    "_http_server",
    "_stream_duplex",
    "_stream_passthrough",
    "_stream_readable",
    "_stream_transform",
    "_stream_wrap",
    "_stream_writable",
    "_tls_common",
    "_tls_wrap",
    "assert",
    "assert/strict",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "dns/promises",
    "domain",
    "events",
    "fs",
    "fs/promises",
    "http",
    "http2",
    "https",
    "inspector",
    "inspector/promises",
    "module",
    "net",
    "os",
    "path",
    "path/posix",
    "path/win32",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "readline/promises",
    "repl",
    "stream",
    "stream/consumers",
    "stream/promises",
    "stream/web",
    "string_decoder",
    "sys",
    "timers",
    "timers/promises",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "util/types",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];

/// Without `Fact::Packages` in the rule's `USES` a declared polyfill reads as `Node` and a
/// workspace package as `External`; without `Fact::Aliases` a key of `paths` reads as `External`.
pub fn import_kind(file: &File<'_>, facts: &dyn Facts, source: &str) -> ImportKind {
    // An npm package name never starts with `.`.
    if source.starts_with('.') || source.starts_with('/') {
        return ImportKind::Relative;
    }
    // Nor with `#`: that is a key of `imports` in the package's own `package.json`.
    if source.starts_with('#') {
        return ImportKind::Alias;
    }
    if source.starts_with("node:") {
        return ImportKind::Node;
    }
    let package = facts.package(file.path);
    let name = package_name(source);
    // A declared package with a builtin's name, like the `events` polyfill, is what a bundler
    // loads for the bare name; `node:` would break the build.
    if BUILTINS.contains(&source) && !package.is_some_and(|p| p.dependencies.contains(name)) {
        return ImportKind::Node;
    }
    // Before aliases: a monorepo package can also be a key of `paths`.
    if package.is_some_and(|p| p.workspace.contains(name)) {
        return ImportKind::Workspace;
    }
    if facts
        .aliases(file.path)
        .iter()
        .any(|key| alias_matches(key, source))
    {
        return ImportKind::Alias;
    }
    ImportKind::External
}

/// `@scope/name/sub` -> `@scope/name`, `name/sub` -> `name`.
pub fn package_name(source: &str) -> &str {
    let end = if source.starts_with('@') {
        source.match_indices('/').nth(1)
    } else {
        source.match_indices('/').next()
    };
    end.map_or(source, |(at, _)| &source[..at])
}

/// The literal that names the module of an import, a re-export, `import('x')`, `import('x').T`,
/// `import x = require('x')` or a call of the global `require('x')`; its span takes the quotes.
pub fn module_source<'a>(file: &File<'a>, kind: AstKind<'a>) -> Option<&'a StringLiteral<'a>> {
    match kind {
        AstKind::ImportDeclaration(import) => Some(&import.source),
        AstKind::ExportFromDeclaration(export) => Some(&export.source),
        AstKind::ExportAllDeclaration(export) => Some(&export.source),
        AstKind::ImportExpression(import) => match &import.source {
            Expression::StringLiteral(source) => Some(source),
            _ => None,
        },
        AstKind::TSImportType(import) => Some(&import.source),
        AstKind::TSExternalModuleReference(reference) => Some(&reference.expression),
        AstKind::CallExpression(call) => match (&call.callee, call.arguments.as_slice()) {
            (Expression::Identifier(callee), [Argument::StringLiteral(source)])
                if callee.name == "require"
                    && file.semantic.is_reference_to_global_variable(callee) =>
            {
                Some(source)
            }
            _ => None,
        },
        _ => None,
    }
}

/// The import a reference names: the module text and the name the module exports it under.
/// `Get` after `import { Get } from '@nestjs/common'` or `import { Get as Route } from ..` gives
/// `("@nestjs/common", "Get")`, a default import `"default"`; `import type` counts too, since a
/// type reference names it. `None` for a namespace import (`common.Get` is a member, not a
/// name), `import x = require(..)`, a declaration of the file and a global.
pub fn imported_name<'a>(
    file: &File<'a>,
    reference: &IdentifierReference<'_>,
) -> Option<(&'a str, &'a str)> {
    let scoping = file.semantic.scoping();
    let symbol = scoping
        .get_reference(reference.reference_id.get()?)
        .symbol_id()?;
    let nodes = file.semantic.nodes();
    let declaration = scoping.symbol_declaration(symbol);
    let name = match nodes.kind(declaration) {
        AstKind::ImportSpecifier(specifier) => specifier.imported.name().as_str(),
        AstKind::ImportDefaultSpecifier(_) => "default",
        _ => return None,
    };
    match nodes.parent_kind(declaration) {
        AstKind::ImportDeclaration(import) => Some((import.source.value.as_str(), name)),
        _ => None,
    }
}

/// The name a reference imports from `package`, from its root or a subpath: `Route` after
/// `import { Get as Route } from '@nestjs/common'` gives `Some("Get")` for `@nestjs/common`.
pub fn imported_from<'a>(
    file: &File<'a>,
    reference: &IdentifierReference<'_>,
    package: &str,
) -> Option<&'a str> {
    let (module, name) = imported_name(file, reference)?;
    (package_name(module) == package).then_some(name)
}

/// A lone `*` is the fallback tsconfig gives every package, so it matches nothing: else any npm
/// import would read as an alias.
fn alias_matches(key: &str, source: &str) -> bool {
    match key.split_once('*') {
        Some(("", "")) => false,
        Some((prefix, suffix)) => source
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.ends_with(suffix)),
        None => key == source,
    }
}
