//! The recipe is the folder of the stand file: the file, the harness and anything else there. The
//! seal of a session compares it file by file.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, BufRead};

use camino::Utf8Path;
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
        for entry in folder.read_dir_utf8().map_err(SpecError::io(&folder))? {
            let entry = entry.map_err(SpecError::io(&folder))?;
            let (name, path) = (entry.file_name(), entry.path());
            let kind = entry.file_type().map_err(SpecError::io(path))?;
            let sha = if kind.is_dir() {
                folders.push((path.to_owned(), format!("{prefix}{name}/")));
                continue;
            } else if kind.is_symlink() {
                let target = fs::read_link(path).map_err(SpecError::io(path))?;
                hex(&Sha256::digest(target.as_os_str().as_encoded_bytes()))
            } else if kind.is_file() {
                file_sha256(path).map_err(SpecError::io(path))?
            } else {
                // Opening a FIFO would wait for a writer.
                return Err(SpecError::io(path)(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "neither a file, a folder nor a link",
                )));
            };
            files.insert(format!("{prefix}{name}"), sha);
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

/// Read in chunks: a data file of the harness may be large.
fn file_sha256(path: &Utf8Path) -> io::Result<String> {
    let mut file = io::BufReader::with_capacity(1 << 16, fs::File::open(path)?);
    let mut hasher = Sha256::new();
    loop {
        let chunk = file.fill_buf()?;
        if chunk.is_empty() {
            return Ok(hex(&hasher.finalize()));
        }
        hasher.update(chunk);
        let read = chunk.len();
        file.consume(read);
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
