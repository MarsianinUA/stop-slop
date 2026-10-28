//! Paths of the guest. The guest runs Linux, so they are POSIX strings on every host: on Windows
//! `Utf8Path` would not take `/w/app/a.ts` for absolute and would spell it with `\`.

use std::borrow::Cow;

const TS_EXTENSIONS: [&str; 4] = ["ts", "tsx", "mts", "cts"];

/// A guest path under `guest_tree` becomes the path from the repo root by prefix; nothing is read
/// on the host by a guest path, because the preload puts every map into `maps.json`.
#[derive(Debug)]
pub struct PathMap {
    /// Normalized, without the trailing `/`: empty for `/`.
    tree: String,
}

impl PathMap {
    pub fn new(guest_tree: &str) -> PathMap {
        PathMap {
            tree: normalize(guest_tree).trim_end_matches('/').to_owned(),
        }
    }

    /// The path from the repo root, `/`-separated, of a guest path that is local: under the tree
    /// and outside `node_modules`. `guest` is normalized, as `guest_path` gives it.
    pub(crate) fn local<'g>(&self, guest: &'g str) -> Option<&'g str> {
        let rest = guest.strip_prefix(&self.tree)?.strip_prefix('/')?;
        if rest.split('/').any(|part| part == "node_modules") {
            return None;
        }
        Some(rest)
    }
}

/// The guest path of a `file://` URL or of a path from `/`; `None` for anything else, such as
/// `node:fs`, `evalmachine.<anonymous>` or the empty URL of V8's own frames. The query of a URL
/// is not part of its path: `import('./a.mjs?v=2')` runs `a.mjs`.
pub(crate) fn guest_path(url: &str) -> Option<String> {
    let path = match url.strip_prefix("file://") {
        Some(rest) => {
            let rest = rest.find(['?', '#']).map_or(rest, |end| &rest[..end]);
            Cow::Owned(percent_decoded(rest)?)
        }
        None => Cow::Borrowed(url),
    };
    path.starts_with('/').then(|| normalize(&path))
}

/// `relative` from the folder of `base`, a guest path.
pub(crate) fn resolve(base: &str, relative: &str) -> Option<String> {
    let (folder, _) = base.rsplit_once('/')?;
    guest_path(&format!("{folder}/{relative}"))
}

/// Without a map, the positions in such a file are not its source's: it runs compiled. The name
/// `.ts` has no extension.
pub(crate) fn is_typescript(guest: &str) -> bool {
    let name = guest.rsplit_once('/').map_or(guest, |(_, name)| name);
    name.rsplit_once('.')
        .is_some_and(|(stem, extension)| !stem.is_empty() && TS_EXTENSIONS.contains(&extension))
}

fn percent_decoded(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = text.get(at + 1..at + 3)?;
            if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return None;
            }
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            decoded.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// A path from `/` without `.`, `..` and empty parts, so one file has one spelling.
fn normalize(path: &str) -> String {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    let mut normal = String::with_capacity(path.len());
    for part in parts {
        normal.push('/');
        normal.push_str(part);
    }
    if normal.is_empty() {
        normal.push('/');
    }
    normal
}
