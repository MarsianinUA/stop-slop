//! The setup cache of a session (plan, row 88): the key of each version after `pull`, the entry
//! `run` takes, and the slots after the run. `run` lays out an entry, `store` keeps the slots;
//! the decisions are here.

use report::SetupFrom;
use run::SetupCache;
use sandbox::{Leftovers, Pulled, Sandbox};
use source::Tree;
use spec::Stand;
use store::{CacheEntry, Session, SetupKey, Slot, Store};

use crate::error::StandError;
use crate::sweep;

/// The entry of one version for this run.
#[derive(Debug, Clone)]
pub(crate) struct Planned {
    pub slot: Slot,
    pub key: SetupKey,
    pub entry: CacheEntry,
    pub hit: bool,
}

impl Planned {
    pub(crate) fn setup(&self) -> SetupCache {
        let dir = self.entry.dir.clone();
        if self.hit {
            SetupCache::Hit(dir)
        } else {
            SetupCache::Build(dir)
        }
    }
}

/// `recipe` is the digest of the recipe the run reads.
pub(crate) fn key(stand: &Stand, tree: &Tree, pulled: &Pulled) -> SetupKey {
    SetupKey {
        tree: tree.to_string(),
        recipe: stand.recipe.digest.clone(),
        images: pulled
            .digests
            .iter()
            .map(|(image, digest)| (image.as_str().to_owned(), digest.clone()))
            .collect(),
        guest: pulled.guest.clone(),
        tool: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

/// A hit needs `entry.json` and every snapshot it names among `left`; otherwise the snapshots
/// still there go, and the folder of the key starts empty for this run's setup.
pub(crate) fn plan(
    session: &Session,
    slot: Slot,
    key: SetupKey,
    left: &Leftovers,
    run: &str,
    sandbox: &impl Sandbox,
) -> Result<Planned, StandError> {
    if let Some(entry) = session.cached(&key)?
        && let Some(ids) = run::cached_ids(&entry.dir)?
    {
        if ids.iter().all(|id| left.snapshots.contains(id)) {
            return Ok(Planned {
                slot,
                key,
                entry,
                hit: true,
            });
        }
        for id in ids.iter().filter(|id| left.snapshots.contains(id)) {
            sandbox.drop_snapshot(id)?;
        }
    }
    let dir = session.new_entry(&key)?;
    let entry = CacheEntry {
        dir,
        built_by: run.to_owned(),
    };
    Ok(Planned {
        slot,
        key,
        entry,
        hit: false,
    })
}

pub(crate) fn setup_from(head: &Planned) -> SetupFrom {
    SetupFrom {
        head: head.hit.then(|| head.entry.built_by.clone()),
        before: None,
    }
}

/// After exit 0 or 1: each slot points at its version's entry when that setup passed whole;
/// the entries no slot names lose their snapshots, then their folders.
pub(crate) fn keep(
    session: &Session,
    plans: &[Planned],
    sandbox: &impl Sandbox,
    store: &Store,
) -> Result<(), StandError> {
    let mut gone = Vec::new();
    for planned in plans {
        if run::cached_ids(&planned.entry.dir)?.is_some() {
            gone.extend(session.cache(planned.slot, &planned.key, planned.entry.clone())?);
        }
    }
    sweep::drop_snapshots(sandbox, &gone, &[])?;
    store.clear_unused_entries()?;
    Ok(())
}

/// After exit 2, 3 or 130 the slots stay; what this run built goes, its snapshots first.
pub(crate) fn discard(
    plans: &[Planned],
    sandbox: &impl Sandbox,
    store: &Store,
) -> Result<(), StandError> {
    let built: Vec<_> = plans
        .iter()
        .filter(|planned| !planned.hit)
        .map(|planned| planned.entry.dir.clone())
        .collect();
    sweep::drop_snapshots(sandbox, &built, &[])?;
    store.clear_unused_entries()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use sandbox::ImageRef;

    use super::*;

    #[test]
    fn the_key_takes_the_tree_the_recipe_each_digest_the_guest_and_the_tool() {
        let dir = tempfile::tempdir().unwrap();
        let file = camino::Utf8Path::from_path(dir.path())
            .unwrap()
            .join("compose.yaml");
        std::fs::write(
            &file,
            "services:\n  w:\n    image: alpine\n    deploy: { resources: { limits: { cpus: \"1\", memory: 64m } } }\n\
             x-stand: { workload: w }\n",
        )
        .unwrap();
        let stand = spec::load(&file, &spec::Policy::new(None).unwrap()).unwrap();
        let pulled = |digest: &str, guest: &str| Pulled {
            digests: vec![(ImageRef::new("alpine"), digest.to_owned())],
            guest: guest.into(),
        };
        // `source` makes a `Tree` only from a repo.
        let repo = testkit::TempRepo::new();
        repo.write("a.txt", "a").commit("a");
        let worktree = source::open(camino::Utf8Path::from_path(repo.path()).unwrap()).unwrap();
        let tree = source::snapshot(&worktree).unwrap();
        let key = key(&stand, &tree, &pulled("sha256:1", "0.1.0"));
        assert_eq!(key.tree, tree.to_string());
        assert_eq!(key.recipe, stand.recipe.digest);
        assert_eq!(key.images, [("alpine".to_owned(), "sha256:1".to_owned())]);
        assert_eq!(key.guest, "0.1.0");
        assert_eq!(key.tool, env!("CARGO_PKG_VERSION"));
        let other_digest = super::key(&stand, &tree, &pulled("sha256:2", "0.1.0"));
        let other_guest = super::key(&stand, &tree, &pulled("sha256:1", "dev-1"));
        assert_ne!(key.digest(), other_digest.digest());
        assert_ne!(key.digest(), other_guest.digest());
    }
}
