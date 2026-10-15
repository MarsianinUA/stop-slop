use std::collections::{BTreeMap, BTreeSet};
use std::panic::resume_unwind;
use std::thread;

use camino::{Utf8Path, Utf8PathBuf};
use kit::serde::de::DeserializeOwned;
use kit::{EnumIndex, Fact, Facts, Package};
use repo::{RepoError, Snapshot};

mod enums;
mod package;
mod tsconfig;

pub use enums::enum_index;

const PACKAGE_JSON: &str = "package.json";
const TSCONFIG_JSON: &str = "tsconfig.json";
const BOM: &[u8] = b"\xEF\xBB\xBF";

/// The facts `load` was asked for; the other ones are empty. `package` and `aliases` answer
/// for the files given to `load` and for any file in a folder on their way to the root; for
/// other files they give `None` and `&[]`.
#[derive(Debug)]
pub struct ProjectFacts {
    /// Keyed by a folder of the repo, `""` for the root.
    folders: BTreeMap<String, Nearest>,
    packages: Vec<Package>,
    aliases: Vec<Vec<String>>,
    enums: EnumIndex,
}

/// Indexes into `packages` and `aliases` of `ProjectFacts`.
#[derive(Debug, Clone, Copy, Default)]
struct Nearest {
    package: Option<usize>,
    aliases: Option<usize>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error(transparent)]
    Repo(#[from] RepoError),
    #[error("{path}: {message}")]
    Json { path: Utf8PathBuf, message: String },
}

impl ProjectFacts {
    /// Builds every fact of `needed` up front: `Facts` is read from rayon tasks, where a lazy
    /// build would block the threads. The enum index does not depend on `files`, so it is
    /// built on its own thread while this one reads `package.json` and `tsconfig.json`.
    pub fn load(
        snapshot: &Snapshot,
        files: &[Utf8PathBuf],
        needed: &[Fact],
    ) -> Result<ProjectFacts, ProjectError> {
        let packages = needed.contains(&Fact::Packages);
        let aliases = needed.contains(&Fact::Aliases);
        let enums_needed = needed.contains(&Fact::Enums);
        let enums = || {
            if enums_needed {
                enums::load(snapshot)
            } else {
                Ok(EnumIndex::new(Vec::new()))
            }
        };
        let by_folder = || by_folder(snapshot, files, packages, aliases);
        let (enums, by_folder) = if enums_needed && (packages || aliases) {
            thread::scope(|scope| {
                let enums = scope.spawn(enums);
                let by_folder = by_folder();
                let enums = enums.join().unwrap_or_else(|panic| resume_unwind(panic));
                (enums, by_folder)
            })
        } else {
            (enums(), by_folder())
        };
        let (enums, (folders, packages, aliases)) = (enums?, by_folder?);
        Ok(ProjectFacts {
            folders,
            packages,
            aliases,
            enums,
        })
    }

    fn nearest(&self, file: &Utf8Path) -> Nearest {
        self.folders
            .get(dir_of(file.as_str()))
            .copied()
            .unwrap_or_default()
    }
}

impl Facts for ProjectFacts {
    fn package(&self, file: &Utf8Path) -> Option<&Package> {
        self.nearest(file)
            .package
            .map(|index| &self.packages[index])
    }

    fn aliases(&self, file: &Utf8Path) -> &[String] {
        self.nearest(file)
            .aliases
            .map_or(&[], |index| &self.aliases[index])
    }

    fn enums(&self) -> &EnumIndex {
        &self.enums
    }
}

type ByFolder = (BTreeMap<String, Nearest>, Vec<Package>, Vec<Vec<String>>);

/// The candidates of every folder go into one `read`: on `Rev` that is one `cat-file`.
fn by_folder(
    snapshot: &Snapshot,
    files: &[Utf8PathBuf],
    packages: bool,
    aliases: bool,
) -> Result<ByFolder, ProjectError> {
    if !packages && !aliases {
        return Ok(Default::default());
    }
    let folders = folders(files);
    let names: Vec<&str> = [(packages, PACKAGE_JSON), (aliases, TSCONFIG_JSON)]
        .into_iter()
        .filter_map(|(wanted, name)| wanted.then_some(name))
        .collect();
    let candidates: Vec<Utf8PathBuf> = folders
        .iter()
        .flat_map(|folder| names.iter().map(move |name| at(folder, name)))
        .collect();
    let bytes = snapshot.read(&candidates)?;
    let mut found = candidates.into_iter().zip(bytes);
    let mut package_list = Vec::new();
    let mut configs = Vec::new();
    // A parent is a prefix of its child, so the set gives it first. The root finds no
    // parent: its `dir_of` is itself.
    let mut nearest: BTreeMap<String, Nearest> = BTreeMap::new();
    for folder in folders {
        let mut near = nearest.get(dir_of(&folder)).copied().unwrap_or_default();
        for (path, bytes) in found.by_ref().take(names.len()) {
            let Some(bytes) = bytes else { continue };
            if path.file_name() == Some(PACKAGE_JSON) {
                near.package = Some(package_list.len());
                package_list.push(package::parse(&path, bytes)?);
            } else {
                // `tsconfig::aliases` answers in the order of `configs`.
                near.aliases = Some(configs.len());
                configs.push((path, bytes));
            }
        }
        nearest.insert(folder, near);
    }
    let alias_list = tsconfig::aliases(snapshot, configs)?;
    Ok((nearest, package_list, alias_list))
}

/// Every folder from the folder of each file up to the root.
fn folders(files: &[Utf8PathBuf]) -> BTreeSet<String> {
    let mut folders = BTreeSet::new();
    for file in files {
        let mut folder = dir_of(file.as_str());
        // A folder already in the set came with all its parents.
        while folders.insert(folder.to_owned()) && !folder.is_empty() {
            folder = dir_of(folder);
        }
    }
    folders
}

/// `""` for a file of the root.
fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// Repo paths are built as strings with `/`: `Utf8Path::join` puts `\` on Windows, and
/// `cat-file` of `Rev` finds no `<oid>:apps\x\package.json`.
fn at(folder: &str, name: &str) -> Utf8PathBuf {
    if folder.is_empty() {
        name.into()
    } else {
        format!("{folder}/{name}").into()
    }
}

/// Reads JSON with comments and trailing commas, as `tsconfig.json` allows.
fn json<T: DeserializeOwned>(path: &Utf8Path, mut bytes: Vec<u8>) -> Result<T, ProjectError> {
    if bytes.starts_with(BOM) {
        bytes.drain(..BOM.len());
    }
    let error = |message: String| ProjectError::Json {
        path: path.to_owned(),
        message,
    };
    // Comments turn into spaces, so the line and column of an error stay right.
    json_strip_comments::strip_slice(&mut bytes).map_err(|e| error(e.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|e| error(e.to_string()))
}
