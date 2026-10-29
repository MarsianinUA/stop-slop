//! What an earlier run left, under the lock (plan, row 30), the TTL, and room on the disk
//! (row 88). The lock proves no run is alive, so every VM of the namespace is a leftover; a
//! snapshot stays only while a kept run or a cache slot of a live session names it.

use std::time::SystemTime;

use budget::{Host, HostError};
use camino::{Utf8Path, Utf8PathBuf};
use sandbox::{ImageRef, Sandbox, SnapshotId};
use store::{KeptScope, RunPaths, Store};

use crate::error::StandError;

/// Step 4 of the call order. `now` is the one `expired` gets, so a session expired at one time
/// and live at the other cannot lose its snapshots and stay. An error reading `kept.json` or
/// `entry.json` is an error, never "nothing kept": a snapshot an agent may still inspect stays.
pub(crate) fn leftovers(
    sandbox: &impl Sandbox,
    store: &Store,
    now: SystemTime,
) -> Result<(), StandError> {
    let keep = named(&store.cache_entries(now)?, &store.kept_runs(now)?)?;
    clear(sandbox, &keep)?;
    store.clear_trees()?;
    store.clear_unused_entries()?;
    let expired = store.expired(now)?;
    store.forget(&expired)?;
    // After the snapshots: a snapshot pins its image (plan, row 87).
    let images: Vec<ImageRef> = store
        .kept_images()?
        .into_iter()
        .map(ImageRef::new)
        .collect();
    sandbox.prune_images(&images)?;
    Ok(())
}

/// Step 5, before the budget: while the disk is low, first the cache and the kept runs of other
/// sessions go, then the cache of this one, then its kept run; the host is read again after each.
/// The cache goes first because it costs only time (plan, row 88).
pub(crate) fn make_room(
    sandbox: &impl Sandbox,
    store: &Store,
    session: &str,
    host: &dyn Fn(&Utf8Path) -> Result<Host, HostError>,
) -> Result<Host, StandError> {
    let tool = &store.paths().root;
    let mut read = host(tool)?;
    let others = || KeptScope::OtherThan(session.to_owned());
    for step in 0..3 {
        if !budget::disk_low(&read) {
            break;
        }
        let (entries, runs) = match step {
            0 => (store.drop_cache(others())?, store.drop_kept(others())?),
            1 => (store.drop_cache(KeptScope::All)?, Vec::new()),
            _ => (Vec::new(), store.drop_kept(KeptScope::All)?),
        };
        drop_snapshots(sandbox, &entries, &runs)?;
        store.clear_unused_entries()?;
        read = host(tool)?;
    }
    Ok(read)
}

/// Every VM of the namespace and every snapshot but `keep`: under the lock no run is alive.
pub(crate) fn clear(sandbox: &impl Sandbox, keep: &[SnapshotId]) -> Result<(), StandError> {
    let left = sandbox.leftovers()?;
    for id in &left.sandboxes {
        sandbox.destroy(id)?;
    }
    for id in left.snapshots.iter().filter(|id| !keep.contains(id)) {
        sandbox.drop_snapshot(id)?;
    }
    Ok(())
}

/// The snapshots of cache entries and of kept runs that nothing names any more.
pub(crate) fn drop_snapshots(
    sandbox: &impl Sandbox,
    entries: &[Utf8PathBuf],
    runs: &[RunPaths],
) -> Result<(), StandError> {
    for id in &named(entries, runs)? {
        sandbox.drop_snapshot(id)?;
    }
    Ok(())
}

fn named(entries: &[Utf8PathBuf], runs: &[RunPaths]) -> Result<Vec<SnapshotId>, StandError> {
    let mut ids = Vec::new();
    for entry in entries {
        ids.extend(run::cached_ids(entry)?.unwrap_or_default());
    }
    for run in runs {
        ids.extend(run::kept_ids(&run.dir)?);
    }
    Ok(ids)
}
