//! The recipe is the folder of the stand file: the file, the harness and anything else there. The
//! seal of a session compares it file by file.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;

use camino::{Utf8Path, Utf8PathBuf};
use sha2::{Digest, Sha256};

use crate::SpecError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    /// sha256 hex of the sorted `path sha256` lines.
    pub digest: String,
    /// Path relative to the folder, `/`-separated -> sha256 hex of the bytes.
    pub files: BTreeMap<String, String>,
}

impl Recipe {
    /// Changed, added and removed paths against the sealed copy, sorted.
    pub fn changed(&self, sealed: &Recipe) -> Vec<String> {
        let paths: BTreeSet<&String> = self.files.keys().chain(sealed.files.keys()).collect();
        paths
            .into_iter()
            .filter(|path| self.files.get(*path) != sealed.files.get(*path))
            .cloned()
            .collect()
    }
}

/// Every file under `dir`. A link is not followed: a bind shows it as a link, so the path it
/// holds is what counts, and a link that loops cannot hang the walk.
pub(crate) fn of(dir: &Utf8Path) -> Result<Recipe, SpecError> {
    let mut files = BTreeMap::new();
    let mut folders = vec![(dir.to_owned(), String::new())];
    while let Some((folder, prefix)) = folders.pop() {
        for entry in fs::read_dir(&folder).map_err(io(&folder))? {
            let entry = entry.map_err(io(&folder))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|name| SpecError::Io {
                    file: folder.clone(),
                    source: io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{} is not UTF-8", name.to_string_lossy()),
                    ),
                })?;
            let path = folder.join(&name);
            let kind = entry.file_type().map_err(io(&path))?;
            let bytes = if kind.is_dir() {
                folders.push((path, format!("{prefix}{name}/")));
                continue;
            } else if kind.is_symlink() {
                let target = fs::read_link(&path).map_err(io(&path))?;
                target.to_string_lossy().into_owned().into_bytes()
            } else {
                fs::read(&path).map_err(io(&path))?
            };
            files.insert(format!("{prefix}{name}"), hex(&Sha256::digest(bytes)));
        }
    }
    let lines: String = files
        .iter()
        .map(|(path, sha)| format!("{path} {sha}\n"))
        .collect();
    Ok(Recipe {
        digest: hex(&Sha256::digest(lines)),
        files,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn io(file: &Utf8Path) -> impl Fn(io::Error) -> SpecError {
    let file: Utf8PathBuf = file.to_owned();
    move |source| SpecError::Io {
        file: file.clone(),
        source,
    }
}
