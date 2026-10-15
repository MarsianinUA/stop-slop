use crate::{Facts, File};

/// Where an import specifier points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportKind {
    /// `node:fs`, or a bare builtin like `fs` that the file's packages do not declare.
    Node,
    External,
    /// A `workspace:` package of the same monorepo, with or without a subpath.
    Workspace,
    /// A key of `paths` in the nearest `tsconfig.json`, like `@/*`.
    Alias,
    /// `./x`, `../x`, `.`, `..`, `/x`.
    Relative,
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
/// workspace package as `External`; without `Fact::Aliases` an alias reads as `External`.
pub fn import_kind(file: &File<'_>, facts: &dyn Facts, source: &str) -> ImportKind {
    // An npm package name never starts with `.`.
    if source.starts_with('.') || source.starts_with('/') {
        return ImportKind::Relative;
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
fn package_name(source: &str) -> &str {
    let end = if source.starts_with('@') {
        source.match_indices('/').nth(1)
    } else {
        source.match_indices('/').next()
    };
    end.map_or(source, |(at, _)| &source[..at])
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
