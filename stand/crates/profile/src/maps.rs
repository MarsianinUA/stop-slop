use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fs;

use camino::Utf8PathBuf;
use oxc_sourcemap::{SourceMap, Token};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::ProfileError;
use crate::paths;

/// The scripts of one process. A map never serves another process: two jest projects may
/// compile one file differently.
pub(crate) struct Scripts {
    entries: HashMap<String, Entry>,
    known: HashMap<String, Script>,
}

pub(crate) struct Script {
    /// The guest path; `None` for `node:` modules and V8's own frames.
    pub(crate) file: Option<String>,
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
            let file = paths::guest_path(url);
            let map = self.entries.remove(url).and_then(Map::load);
            self.known.insert(url.to_owned(), Script { file, map });
        }
        &self.known[url]
    }
}

/// The value of a script in `<id>.<n>.maps.json`.
#[derive(Deserialize)]
struct Entry {
    /// The script's map without `sourcesContent`, inline or from the `.map` file it names. Parsed
    /// only when the script is resolved: a map that does not parse leaves its script unmapped
    /// instead of failing the read.
    map: Box<RawValue>,
    /// The guest path relative sources start from: the script for an inline map, else the `.map`
    /// file.
    base: String,
}

pub(crate) struct Map {
    tokens: Box<[Token]>,
    /// Guest paths; `None` for a source that is not a file, such as `webpack://app/src/a.ts`.
    sources: Vec<Option<String>>,
}

impl Map {
    /// `None` when the map does not parse.
    fn load(entry: Entry) -> Option<Map> {
        let parts = SourceMap::from_json_string(entry.map.get())
            .ok()?
            .into_parts();
        let root = parts.source_root.as_deref().filter(|root| !root.is_empty());
        let sources = parts
            .sources
            .iter()
            .map(|source| source_path(source, root, &entry.base))
            .collect();
        let mut tokens = parts.tokens;
        // The lookups search by position, and `mappings` may list a line's segments in any order.
        tokens.sort_by_key(|token| (token.get_dst_line(), token.get_dst_col()));
        Some(Map { tokens, sources })
    }

    /// The original file, line and column (from 0) of generated `line`:`column`: the last
    /// mapping at or before it, on that line or an earlier one, as Node's `findEntry` picks it.
    pub(crate) fn find(&self, line: u32, column: u32) -> Option<(&str, u32, u32)> {
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
    pub(crate) fn line(&self, line: u32) -> Option<(&str, u32)> {
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
    pub(crate) fn module(&self) -> Option<&str> {
        self.tokens.iter().find_map(|token| self.source(token))
    }

    fn source(&self, token: &Token) -> Option<&str> {
        let id = usize::try_from(token.get_source_id()?).ok()?;
        self.sources.get(id)?.as_deref()
    }
}

/// `source` after `sourceRoot`, from the folder of the map, as the source map spec resolves it.
fn source_path(source: &str, root: Option<&str>, base: &str) -> Option<String> {
    let source = match root {
        Some(root) => Cow::Owned(format!("{}/{source}", root.trim_end_matches('/'))),
        None => Cow::Borrowed(source),
    };
    if let Some(path) = paths::guest_path(&source) {
        return Some(path);
    }
    if has_scheme(&source) {
        return None;
    }
    paths::resolve(base, &source)
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
