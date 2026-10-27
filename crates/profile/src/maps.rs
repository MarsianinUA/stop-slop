use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fs;

use camino::{Utf8Component, Utf8Path, Utf8PathBuf, Utf8Prefix};
use oxc_sourcemap::{SourceMap, Token};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::ProfileError;

/// The scripts of one process. A map never serves another process: two jest projects may
/// compile one file differently.
pub(crate) struct Scripts {
    entries: HashMap<String, Entry>,
    known: HashMap<String, Script>,
}

pub(crate) struct Script {
    /// `None` for `node:` modules and V8's own frames.
    pub(crate) file: Option<Utf8PathBuf>,
    pub(crate) map: Option<Map>,
}

impl Scripts {
    pub(crate) fn load(maps: &BTreeMap<u32, Utf8PathBuf>) -> Result<Scripts, ProfileError> {
        let mut entries = HashMap::new();
        for path in maps.values() {
            let bytes = fs::read(path).map_err(ProfileError::io(path))?;
            let chunk: HashMap<String, Entry> = serde_json::from_slice(&bytes)
                .map_err(|error| ProfileError::bad(path, error.to_string()))?;
            entries.extend(chunk);
        }
        Ok(Scripts {
            entries,
            known: HashMap::new(),
        })
    }

    pub(crate) fn get(&mut self, url: &str) -> &Script {
        if !self.known.contains_key(url) {
            let file = file_path(url);
            let map = file.as_deref().and_then(|file| {
                let entry = self.entries.remove(url)?;
                Map::load(entry, file)
            });
            self.known.insert(url.to_owned(), Script { file, map });
        }
        &self.known[url]
    }
}

/// The value of a script in `<id>.<n>.maps.json`.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Entry {
    /// The inline `data:` map of the script, without `sourcesContent`. Parsed only when the
    /// script is resolved: a map that does not parse leaves its script unmapped instead of
    /// failing the read.
    Map(Box<RawValue>),
    /// The `.map` file the script's `sourceMappingURL` names.
    File(Utf8PathBuf),
}

pub(crate) struct Map {
    tokens: Box<[Token]>,
    /// `None` for a source that is not a file, such as `webpack://app/src/a.ts`.
    sources: Vec<Option<Utf8PathBuf>>,
}

impl Map {
    /// `None` when the map does not parse or its file cannot be read. Relative sources of an
    /// inline map start from the folder of `script`.
    fn load(entry: Entry, script: &Utf8Path) -> Option<Map> {
        match entry {
            Entry::Map(raw) => Map::new(SourceMap::from_json_string(raw.get()).ok()?, script),
            Entry::File(path) => {
                let text = fs::read_to_string(&path).ok()?;
                Map::new(SourceMap::from_json_string(&text).ok()?, &path)
            }
        }
    }

    fn new(map: SourceMap<'_>, from: &Utf8Path) -> Option<Map> {
        let parts = map.into_parts();
        let dir = from.parent()?;
        let root = parts.source_root.as_deref().filter(|root| !root.is_empty());
        let sources = parts
            .sources
            .iter()
            .map(|source| source_path(source, root, dir))
            .collect();
        let mut tokens = parts.tokens;
        // The lookups search by position, and `mappings` may list a line's segments in any order.
        tokens.sort_by_key(|token| (token.get_dst_line(), token.get_dst_col()));
        Some(Map { tokens, sources })
    }

    /// The original file, line and column (from 0) of generated `line`:`column`: the last
    /// mapping at or before it, on that line or an earlier one, as Node's `findEntry` picks it.
    pub(crate) fn find(&self, line: u32, column: u32) -> Option<(&Utf8Path, u32, u32)> {
        let after = self
            .tokens
            .partition_point(|token| (token.get_dst_line(), token.get_dst_col()) <= (line, column));
        let token = self.tokens.get(after.checked_sub(1)?)?;
        Some((
            self.source(token)?,
            token.get_src_line(),
            token.get_src_col(),
        ))
    }

    /// The original file and line of generated `line`, from 0: its first mapping, not the one
    /// before it, since a line starts with indentation that has none.
    pub(crate) fn line(&self, line: u32) -> Option<(&Utf8Path, u32)> {
        let first = self
            .tokens
            .partition_point(|token| token.get_dst_line() < line);
        let token = match self.tokens.get(first) {
            Some(token) if token.get_dst_line() == line => token,
            _ => self.tokens.get(first.checked_sub(1)?)?,
        };
        Some((self.source(token)?, token.get_src_line()))
    }

    /// The file the script compiles: the source of its first mapping.
    pub(crate) fn module(&self) -> Option<&Utf8Path> {
        self.tokens.iter().find_map(|token| self.source(token))
    }

    fn source(&self, token: &Token) -> Option<&Utf8Path> {
        let id = usize::try_from(token.get_source_id()?).ok()?;
        self.sources.get(id)?.as_deref()
    }
}

/// `source` after `sourceRoot`, from the folder of the map, as the source map spec resolves it.
fn source_path(source: &str, root: Option<&str>, dir: &Utf8Path) -> Option<Utf8PathBuf> {
    let source = match root {
        Some(root) => Cow::Owned(format!("{}/{source}", root.trim_end_matches('/'))),
        None => Cow::Borrowed(source),
    };
    if let Some(path) = file_path(&source) {
        return Some(path);
    }
    if has_scheme(&source) {
        return None;
    }
    Some(normalize(&dir.join(&*source)))
}

/// `webpack://` or `http:`; a drive letter is not a scheme.
fn has_scheme(source: &str) -> bool {
    source.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() > 1
            && scheme.starts_with(|ch: char| ch.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
    })
}

/// The path of a `file://` URL or of an absolute path; `None` for anything else, such as
/// `node:fs`, `evalmachine.<anonymous>` or the empty URL of V8's own frames. The query of a URL
/// is not part of its path: `import('./a.mjs?v=2')` runs `a.mjs`.
fn file_path(url: &str) -> Option<Utf8PathBuf> {
    let path = match url.strip_prefix("file://") {
        Some(rest) => {
            let rest = rest.find(['?', '#']).map_or(rest, |end| &rest[..end]);
            url_path(&percent_decoded(rest)?)?
        }
        None if Utf8Path::new(url).is_absolute() => url.to_owned(),
        None => return None,
    };
    Some(normalize(Utf8Path::new(&path)))
}

/// `/C:/a` is `C:/a` on Windows, and `server/share/a` there is a share; elsewhere only a path
/// from `/` has a meaning.
fn url_path(rest: &str) -> Option<String> {
    if cfg!(windows) {
        Some(match rest.strip_prefix('/') {
            Some(local) if local.as_bytes().get(1) == Some(&b':') => local.to_owned(),
            Some(_) => rest.to_owned(),
            None => format!("//{rest}"),
        })
    } else {
        rest.starts_with('/').then(|| rest.to_owned())
    }
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

/// Without `.` and `..`, with the separators of the platform and an uppercase drive letter, so
/// one file has one spelling. The disk is not asked: a source may be gone.
pub(crate) fn normalize(path: &Utf8Path) -> Utf8PathBuf {
    let mut normal = Utf8PathBuf::new();
    for component in path.components() {
        match component {
            Utf8Component::CurDir => {}
            Utf8Component::ParentDir => {
                normal.pop();
            }
            Utf8Component::Prefix(prefix) => match prefix.kind() {
                Utf8Prefix::Disk(letter) => {
                    normal.push(format!("{}:", char::from(letter.to_ascii_uppercase())));
                }
                _ => normal.push(prefix.as_str()),
            },
            other => normal.push(other.as_str()),
        }
    }
    normal
}
