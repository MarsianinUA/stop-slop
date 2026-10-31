//! What an earlier run left, under the lock (plan, row 30), the TTL, and room on the disk
//! (row 88). The lock proves no run is alive, so every VM of the namespace is a leftover; a
//! snapshot stays only while a kept run or a cache slot of a live session names it. `--dry-run`
//! drops nothing: it prints what a run would drop (orchestrator, review of T-14).

use std::time::SystemTime;

use budget::{Host, HostError};
use camino::{Utf8Path, Utf8PathBuf};
use sandbox::{ImageRef, Leftovers, Sandbox, SnapshotId};
use store::{Expired, KeptScope, RunPaths, Store};

use crate::error::StandError;

/// Step 4 of the call order. `now` is the one `expired` gets, so a session expired at one time
/// and live at the other cannot lose its snapshots and stay. An error reading `kept.json` or
/// `entry.json` is an error, never "nothing kept": a snapshot an agent may still inspect stays.
pub(crate) fn leftovers(
    sandbox: &impl Sandbox,
    store: &Store,
    now: SystemTime,
) -> Result<(), StandError> {
    let (left, expired) = swept(sandbox, store, now)?;
    clear(sandbox, &left)?;
    store.clear_trees()?;
    store.clear_unused_entries()?;
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

/// The VMs and snapshots `leftovers` would drop now, and the sessions it would forget. The images
/// it prunes are not counted: the port lists none.
pub(crate) fn would_sweep(
    sandbox: &impl Sandbox,
    store: &Store,
    now: SystemTime,
) -> Result<Vec<String>, StandError> {
    let (left, expired) = swept(sandbox, store, now)?;
    let mut lines = Vec::new();
    if !left.sandboxes.is_empty() {
        let vms = count(left.sandboxes.len(), "VM", "VMs");
        lines.push(format!("would destroy {vms} that earlier runs left"));
    }
    if !left.snapshots.is_empty() {
        let snapshots = count(left.snapshots.len(), "snapshot", "snapshots");
        lines.push(format!(
            "would drop {snapshots} that no kept run or cache entry names"
        ));
    }
    if !expired.is_empty() {
        let sessions = count(expired.len(), "expired session", "expired sessions");
        lines.push(format!("would forget the runs and cache of {sessions}"));
    }
    Ok(lines)
}

fn swept(
    sandbox: &impl Sandbox,
    store: &Store,
    now: SystemTime,
) -> Result<(Leftovers, Vec<Expired>), StandError> {
    let entries = store.cache_entries(&KeptScope::All, now)?;
    let keep = named(&entries, &store.kept_runs(&KeptScope::All, now)?)?;
    let mut left = sandbox.leftovers()?;
    left.snapshots.retain(|id| !keep.contains(id));
    Ok((left, store.expired(now)?))
}

/// What a low disk takes, in turn (plan, row 88): the cache and the kept runs of other sessions,
/// then the cache of this one, then its kept run. The cache goes first because it costs only
/// time.
fn room_steps(session: &str) -> [(Option<KeptScope>, Option<KeptScope>, &'static str); 3] {
    let others = KeptScope::OtherThan(session.to_owned());
    let own = KeptScope::Only(session.to_owned());
    [
        (Some(others.clone()), Some(others), "other sessions"),
        (Some(own.clone()), None, "this session"),
        (None, Some(own), "this session"),
    ]
}

/// Step 5, when the budget refuses only the disk of `host`: the steps of `room_steps` while the
/// disk is low, the host read again after each.
pub(crate) fn make_room(
    sandbox: &impl Sandbox,
    store: &Store,
    session: &str,
    host: &dyn Fn(&Utf8Path) -> Result<Host, HostError>,
    low: Host,
) -> Result<Host, StandError> {
    let mut read = low;
    for (cache, kept, _) in room_steps(session) {
        if !budget::disk_low(&read) {
            break;
        }
        let entries = match cache {
            Some(scope) => store.drop_cache(scope)?,
            None => Vec::new(),
        };
        let runs = match kept {
            Some(scope) => store.drop_kept(scope)?,
            None => Vec::new(),
        };
        drop_snapshots(sandbox, &entries, &runs)?;
        store.clear_unused_entries()?;
        read = host(&store.paths().root)?;
    }
    Ok(read)
}

/// What each step of `make_room` would drop, after the sweep at `now`; empty when nothing would.
pub(crate) fn would_make_room(
    store: &Store,
    session: &str,
    now: SystemTime,
) -> Result<Vec<String>, StandError> {
    let mut lines = Vec::new();
    for (cache, kept, whose) in room_steps(session) {
        let entries = match &cache {
            Some(scope) => store.cache_entries(scope, now)?.len(),
            None => 0,
        };
        let runs = match &kept {
            Some(scope) => store.kept_runs(scope, now)?,
            None => Vec::new(),
        };
        let mut what = Vec::new();
        if entries > 0 {
            what.push(count(entries, "cache entry", "cache entries"));
        }
        if !runs.is_empty() {
            let ids: Vec<&str> = runs.iter().map(|run| run.id.as_str()).collect();
            let kept = count(runs.len(), "kept run", "kept runs");
            what.push(format!("{kept} ({})", ids.join(", ")));
        }
        if !what.is_empty() {
            lines.push(format!("would drop {} of {whose}", what.join(" and ")));
        }
    }
    Ok(lines)
}

/// Every VM and every snapshot of `left`: under the lock no run is alive.
pub(crate) fn clear(sandbox: &impl Sandbox, left: &Leftovers) -> Result<(), StandError> {
    for id in &left.sandboxes {
        sandbox.destroy(id)?;
    }
    for id in &left.snapshots {
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

pub(crate) fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
