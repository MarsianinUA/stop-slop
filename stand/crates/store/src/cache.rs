//! The setup cache of a session: the slots `head` and `before` in `session.json`, each naming at
//! most one entry, a folder `cache/<digest>/` that `run` fills. An entry no slot names goes, its
//! snapshots first: `cli` drops them, then `clear_unused_entries` the folder.

use std::collections::BTreeSet;
use std::fmt::Write;
use std::fs;
use std::time::SystemTime;

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::session::{Session, folders, is_expired};
use crate::{KeptScope, Store, StoreError, hex, remove};

const CACHE: &str = "cache";

/// What a setup's result depends on inside the stand; any change misses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupKey {
    pub tree: String,
    /// The `digest` of the recipe the run uses: the sealed copy with `--before`, the live folder
    /// otherwise.
    pub recipe: String,
    /// Each image and the digest this run's `pull` resolved it to.
    pub images: Vec<(String, String)>,
    /// `Pulled.guest`.
    pub guest: String,
    /// The version of the `cli` package.
    pub tool: String,
}

impl SetupKey {
    /// sha256 hex over its parts, one per line; also the name of the entry folder.
    pub fn digest(&self) -> String {
        let mut lines = format!("{}\n{}\n", self.tree, self.recipe);
        for (image, digest) in &self.images {
            writeln!(lines, "{image} {digest}").expect("a String takes any write");
        }
        writeln!(lines, "{}\n{}", self.guest, self.tool).expect("a String takes any write");
        hex(&Sha256::digest(lines))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Head,
    Before,
}

/// `built_by`: the run that built it, `setup.cached` of the reports of later runs.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub dir: Utf8PathBuf,
    pub built_by: String,
}

/// `cache` of `session.json`.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Slots {
    head: Option<Named>,
    before: Option<Named>,
}

/// A slot that names an entry; its folder is `cache/<digest>/`.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Named {
    key: SetupKey,
    digest: String,
    built_by: String,
}

impl Slots {
    fn named(&self) -> impl Iterator<Item = &Named> {
        self.head.iter().chain(&self.before)
    }

    pub(crate) fn images(&self) -> impl Iterator<Item = &String> {
        self.named()
            .flat_map(|named| named.key.images.iter().map(|(image, _)| image))
    }
}

impl Session {
    /// The entry of either slot whose key is `key`.
    pub fn cached(&self, key: &SetupKey) -> Result<Option<CacheEntry>, StoreError> {
        let state = self.state()?;
        let named = state.cache.named().find(|named| named.key == *key);
        Ok(named.map(|named| CacheEntry {
            dir: self.entry(&named.digest),
            built_by: named.built_by.clone(),
        }))
    }

    /// The empty folder `cache/<digest>/` for a setup that runs now; whatever was in it goes.
    pub fn new_entry(&self, key: &SetupKey) -> Result<Utf8PathBuf, StoreError> {
        let dir = self.entry(&key.digest());
        remove(&dir)?;
        fs::create_dir_all(&dir).map_err(StoreError::io(&dir))?;
        Ok(dir)
    }

    /// Points `slot` at `entry`, the folder of `key` that `cached` or `new_entry` gave; returns
    /// the entries no slot names any more, whose snapshots `cli` drops.
    pub fn cache(
        &self,
        slot: Slot,
        key: &SetupKey,
        entry: CacheEntry,
    ) -> Result<Vec<Utf8PathBuf>, StoreError> {
        let mut state = self.state()?;
        let named = Named {
            key: key.clone(),
            digest: key.digest(),
            built_by: entry.built_by,
        };
        let place = match slot {
            Slot::Head => &mut state.cache.head,
            Slot::Before => &mut state.cache.before,
        };
        let replaced = place.replace(named);
        self.write(&state)?;
        Ok(replaced
            .filter(|replaced| {
                state
                    .cache
                    .named()
                    .all(|named| named.digest != replaced.digest)
            })
            .map(|replaced| self.entry(&replaced.digest))
            .into_iter()
            .collect())
    }

    fn entry(&self, digest: &str) -> Utf8PathBuf {
        self.dir.join(CACHE).join(digest)
    }

    fn entries(&self, slots: &Slots) -> BTreeSet<Utf8PathBuf> {
        slots
            .named()
            .map(|named| self.entry(&named.digest))
            .collect()
    }
}

impl Store {
    /// The entries the cache slots of every session not expired at `now` name; the sweep leaves
    /// their snapshots (`run::cached_ids`), so an expired session's snapshots go before `forget`
    /// deletes the folders.
    pub fn cache_entries(&self, now: SystemTime) -> Result<Vec<Utf8PathBuf>, StoreError> {
        let mut entries = Vec::new();
        for (session, state) in self.sessions()? {
            if !is_expired(&state, now) {
                entries.extend(session.entries(&state.cache));
            }
        }
        Ok(entries)
    }

    /// Deletes the entry folders no slot names; `cli` drops their snapshots first.
    pub fn clear_unused_entries(&self) -> Result<(), StoreError> {
        for (session, state) in self.sessions()? {
            let named = session.entries(&state.cache);
            for digest in folders(&session.dir.join(CACHE))? {
                let dir = session.entry(&digest);
                if !named.contains(&dir) {
                    remove(&dir)?;
                }
            }
        }
        Ok(())
    }

    /// Forgets the cache slots of other sessions, or of all, and returns their entries for
    /// `run::cached_ids` and `drop_snapshot`.
    pub fn drop_cache(&self, scope: KeptScope) -> Result<Vec<Utf8PathBuf>, StoreError> {
        let mut dropped = Vec::new();
        for (session, mut state) in self.sessions()? {
            let entries = session.entries(&state.cache);
            if scope.covers(session.key()) && !entries.is_empty() {
                state.cache = Slots::default();
                session.write(&state)?;
                dropped.extend(entries);
            }
        }
        Ok(dropped)
    }
}
