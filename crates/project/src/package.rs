use std::collections::BTreeMap;

use camino::Utf8Path;
use kit::Package;
use kit::serde::Deserialize;

use crate::ProjectError;

/// A value that is not a string is an error, as for npm: skipping the dependency would let
/// `node-protocol --fix` break the import of a polyfill.
#[derive(Deserialize, Default)]
#[serde(crate = "kit::serde", rename_all = "camelCase", default)]
struct PackageJson {
    dependencies: BTreeMap<String, String>,
    dev_dependencies: BTreeMap<String, String>,
    peer_dependencies: BTreeMap<String, String>,
    optional_dependencies: BTreeMap<String, String>,
}

pub(crate) fn parse(path: &Utf8Path, bytes: Vec<u8>) -> Result<Package, ProjectError> {
    let json: PackageJson = crate::json(path, bytes)?;
    let kinds = [
        json.dependencies,
        json.dev_dependencies,
        json.peer_dependencies,
        json.optional_dependencies,
    ];
    let mut package = Package {
        dependencies: Default::default(),
        workspace: Default::default(),
    };
    for (name, version) in kinds.into_iter().flatten() {
        if version.starts_with("workspace:") {
            package.workspace.insert(name.clone());
        }
        package.dependencies.insert(name);
    }
    Ok(package)
}
