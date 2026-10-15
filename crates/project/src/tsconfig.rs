use std::collections::{BTreeMap, BTreeSet, HashMap};

use camino::{Utf8Path, Utf8PathBuf};
use kit::serde::de::IgnoredAny;
use kit::serde::{Deserialize, Deserializer};
use repo::Snapshot;

use crate::ProjectError;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", rename_all = "camelCase")]
struct TsConfig {
    extends: Option<Extends>,
    /// `null` counts as missing, as in TS.
    compiler_options: Option<CompilerOptions>,
}

#[derive(Deserialize)]
#[serde(crate = "kit::serde", untagged)]
enum Extends {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
#[serde(crate = "kit::serde")]
struct CompilerOptions {
    /// The outer `None` is a missing key, the inner one is `null`: in TS a `null` drops the
    /// inherited `paths`.
    #[serde(default, deserialize_with = "present")]
    paths: Option<Option<BTreeMap<String, IgnoredAny>>>,
}

/// Plain `Option` reads `null` as a missing key.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    value: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(value).map(Some)
}

struct Config {
    /// Sorted keys of `paths`, none for `null`; `None` when the config has no `paths` key.
    paths: Option<Vec<String>>,
    /// Repo paths of the local `extends`, in their order.
    extends: Vec<Utf8PathBuf>,
}

/// The keys of `paths` of each config in `found`, in its order. Configs without `paths`
/// take them from their local `extends`; their targets are read one level at a time, all
/// configs at once, and each config is read and resolved once.
pub(crate) fn aliases(
    snapshot: &Snapshot,
    found: Vec<(Utf8PathBuf, Vec<u8>)>,
) -> Result<Vec<Vec<String>>, ProjectError> {
    let roots: Vec<Utf8PathBuf> = found.iter().map(|(path, _)| path.clone()).collect();
    let mut configs: HashMap<Utf8PathBuf, Option<Config>> = HashMap::new();
    let mut level: Vec<(Utf8PathBuf, Option<Vec<u8>>)> = found
        .into_iter()
        .map(|(path, bytes)| (path, Some(bytes)))
        .collect();
    while !level.is_empty() {
        let mut targets = BTreeSet::new();
        for (path, bytes) in level {
            let config = bytes.map(|bytes| parse(&path, bytes)).transpose()?;
            if let Some(config) = &config
                && config.paths.is_none()
            {
                targets.extend(config.extends.iter().cloned());
            }
            configs.insert(path, config);
        }
        let targets: Vec<Utf8PathBuf> = targets
            .into_iter()
            .filter(|target| !configs.contains_key(target))
            .collect();
        let bytes = snapshot.read(&targets)?;
        level = targets.into_iter().zip(bytes).collect();
    }
    let mut resolved = HashMap::new();
    Ok(roots
        .iter()
        .map(|root| {
            paths_of(root, &configs, &mut resolved)
                .map(<[String]>::to_vec)
                .unwrap_or_default()
        })
        .collect())
}

fn parse(path: &Utf8Path, bytes: Vec<u8>) -> Result<Config, ProjectError> {
    let json: TsConfig = crate::json(path, bytes)?;
    let extends = match json.extends {
        None => Vec::new(),
        Some(Extends::One(one)) => vec![one],
        Some(Extends::Many(many)) => many,
    };
    let dir = crate::dir_of(path.as_str());
    Ok(Config {
        paths: json
            .compiler_options
            .and_then(|options| options.paths)
            .map(|paths| paths.map_or_else(Vec::new, |paths| paths.into_keys().collect())),
        extends: extends
            .iter()
            .filter_map(|extends| resolve(dir, extends))
            .map(Utf8PathBuf::from)
            .collect(),
    })
}

/// Own `paths` replace the inherited ones whole, as in TS; of several `extends` the last one
/// with `paths` wins. A missing config or a cycle gives `None`.
fn paths_of<'c>(
    path: &Utf8Path,
    configs: &'c HashMap<Utf8PathBuf, Option<Config>>,
    resolved: &mut HashMap<Utf8PathBuf, Option<&'c [String]>>,
) -> Option<&'c [String]> {
    if let Some(&known) = resolved.get(path) {
        return known;
    }
    // Set before the walk: a cycle back to this config finds `None` and stops.
    resolved.insert(path.to_owned(), None);
    let config = configs.get(path)?.as_ref()?;
    let paths = match &config.paths {
        Some(paths) => Some(paths.as_slice()),
        None => config
            .extends
            .iter()
            .rev()
            .find_map(|target| paths_of(target, configs, resolved)),
    };
    resolved.insert(path.to_owned(), paths);
    paths
}

/// The repo path of a local `extends` of a config in `dir`, or `None` for a package, an
/// absolute path or a path out of the repo. TS turns `\` into `/` first. It also tries the
/// path as is before adding `.json`, but a config without the extension is rare, and the
/// second candidate would cost one more read.
fn resolve(dir: &str, extends: &str) -> Option<String> {
    let extends = extends.replace('\\', "/");
    if !extends.starts_with("./") && !extends.starts_with("../") {
        return None;
    }
    let mut parts: Vec<&str> = dir.split('/').filter(|part| !part.is_empty()).collect();
    for part in extends.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    let mut path = parts.join("/");
    if !path.ends_with(".json") {
        path.push_str(".json");
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_follows_the_extends_from_the_folder_of_the_config() {
        assert_eq!(
            resolve("apps/x", "../../base.json").as_deref(),
            Some("base.json")
        );
        assert_eq!(
            resolve("apps/x", "./a/../b.json").as_deref(),
            Some("apps/x/b.json")
        );
        assert_eq!(resolve("", "./base").as_deref(), Some("base.json"));
        assert_eq!(
            resolve("apps/x", "..\\base.json").as_deref(),
            Some("apps/base.json")
        );
    }

    #[test]
    fn resolve_skips_what_is_not_a_local_file_of_the_repo() {
        assert_eq!(resolve("apps", "../../out.json"), None);
        assert_eq!(resolve("apps/x", "@tsconfig/node20/tsconfig.json"), None);
        assert_eq!(resolve("apps/x", "/abs.json"), None);
        assert_eq!(resolve("apps/x", "C:/abs.json"), None);
    }
}
