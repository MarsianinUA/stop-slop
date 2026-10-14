use kit::oxc::ast::ast::{Argument, Expression, StringLiteral};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

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

impl Rule for Check {
    const USES: &'static [Fact] = &[Fact::Packages];
    const NODES: &'static [AstType] = &[
        AstType::ImportDeclaration,
        AstType::ExportFromDeclaration,
        AstType::ExportAllDeclaration,
        AstType::ImportExpression,
        AstType::TSImportType,
        AstType::TSExternalModuleReference,
        AstType::CallExpression,
    ];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let Some(source) = module_source(node.kind(), file) else {
            return;
        };
        let module = source.value.as_str();
        if !BUILTINS.contains(&module) {
            return;
        }
        // A declared package with a builtin's name, like the `events` polyfill, is what a
        // bundler loads for the bare name; `node:` would break the build.
        let package = module.split_once('/').map_or(module, |(name, _)| name);
        if cx
            .facts()
            .package(file.path)
            .is_some_and(|p| p.dependencies.contains(package))
        {
            return;
        }
        cx.report(source.span).arg("module", module).fix([Edit {
            span: Span::empty(source.span.start + 1),
            text: "node:".into(),
        }]);
    }
}

/// The literal that names the module; its span takes the quotes too.
fn module_source<'a>(kind: AstKind<'a>, file: &File<'a>) -> Option<&'a StringLiteral<'a>> {
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
