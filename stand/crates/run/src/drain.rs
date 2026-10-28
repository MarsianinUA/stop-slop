//! The end of a phase: the cut, the clean stop of every long-running service, the sizes before a
//! snapshot, then the drain (logs, the profile chunks by one `copy_out`) and `destroy`; after it
//! a VM gives nothing, so the profiles are read on the host.

use std::collections::BTreeMap;
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use profile::{Coverage, PathMap, ProfileError};
use report::ProfileOutcome;
use sandbox::{Metrics, Mount, Sandbox, SandboxError, SandboxId, SnapshotId, Stopped};
use serde::Serialize;
use spec::ServiceKind;

use crate::StandFault;
use crate::launch::PROFILE;
use crate::ready::{Fleet, Member};

/// One member at the cut: the end of the measured window.
pub(crate) struct Cut {
    pub(crate) metrics: Metrics,
    /// Bytes of its log so far: `logs` only grows until `destroy`.
    pub(crate) log: u64,
}

/// `metrics` and the log length of every member, by index, before any probe or stop.
pub(crate) fn cut(sandbox: &impl Sandbox, fleet: &Fleet) -> Result<Vec<Cut>, SandboxError> {
    let mut cut = Vec::with_capacity(fleet.members.len());
    for member in &fleet.members {
        cut.push(Cut {
            metrics: sandbox.metrics(&member.id)?,
            log: sandbox.logs(&member.id)?.len() as u64,
        });
    }
    Ok(cut)
}

/// `stop_signal`, `stop_grace`, a kill, for every long-running member, dependents first; by index.
pub(crate) fn stop(
    sandbox: &impl Sandbox,
    fleet: &Fleet,
) -> Result<BTreeMap<usize, Stopped>, SandboxError> {
    let mut stopped = BTreeMap::new();
    for (index, member) in fleet.members.iter().enumerate().rev() {
        if member.kind == ServiceKind::LongRunning {
            stopped.insert(index, sandbox.stop(&member.id)?);
        }
    }
    Ok(stopped)
}

/// The used bytes of each volume by `du` in the guest, then the snapshot with FITRIM in the
/// adapter; `stop` came first, so a database leaves no recovery for the restore.
pub(crate) fn snapshot(
    sandbox: &impl Sandbox,
    id: &SandboxId,
) -> Result<(BTreeMap<String, u64>, SnapshotId), SandboxError> {
    let sizes = sandbox.volume_sizes(id)?;
    let snapshot = sandbox.snapshot(id)?;
    let sizes = sizes
        .into_iter()
        .map(|(name, bytes)| (name.to_string(), bytes));
    Ok((sizes.collect(), snapshot))
}

pub(crate) fn has_volume(member: &Member) -> bool {
    member
        .launch
        .mounts
        .iter()
        .any(|mount| matches!(mount, Mount::Volume { .. }))
}

pub(crate) fn is_profiled(member: &Member) -> bool {
    let mut mounts = member.launch.mounts.iter();
    mounts.any(|mount| matches!(mount, Mount::Scratch { guest } if guest == PROFILE))
}

pub(crate) fn log_path(dir: &Utf8Path, member: &Member) -> Utf8PathBuf {
    dir.join(format!(
        "{}-{}.log",
        member.launch.service, member.launch.replica
    ))
}

fn profile_dir(dir: &Utf8Path, service: &str, replica: u32) -> Utf8PathBuf {
    dir.join("profile").join(format!("{service}-{replica}"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InstanceMetrics<'a> {
    service: &'a str,
    replica: u32,
    metrics: &'a Metrics,
    stopped: Option<&'a Stopped>,
    log_cut: u64,
}

/// `<dir>/metrics.json`: what the cut read and how each service stopped, for a person or an
/// agent looking at the run folder; `report` takes the same from `Raw`.
pub(crate) fn write_metrics(
    dir: &Utf8Path,
    fleet: &Fleet,
    cut: &[Cut],
    stopped: &BTreeMap<usize, Stopped>,
) -> Result<(), StandFault> {
    let instances: Vec<InstanceMetrics> = fleet
        .members
        .iter()
        .zip(cut)
        .enumerate()
        .map(|(index, (member, cut))| InstanceMetrics {
            service: &member.launch.service,
            replica: member.launch.replica,
            metrics: &cut.metrics,
            stopped: stopped.get(&index),
            log_cut: cut.log,
        })
        .collect();
    let path = dir.join("metrics.json");
    let bytes = serde_json::to_vec_pretty(&instances).expect("metrics serialize");
    fs::write(&path, bytes).map_err(StandFault::io(&path))
}

/// Logs, the profile chunks of a profiled member, then `destroy`, member by member: on an error
/// the members not yet destroyed stay in `fleet` for `teardown`.
pub(crate) fn drain(
    sandbox: &impl Sandbox,
    fleet: &mut Fleet,
    dir: &Utf8Path,
) -> Result<(), StandFault> {
    while let Some(member) = fleet.members.first() {
        let log = log_path(dir, member);
        let bytes = sandbox.logs(&member.id).map_err(StandFault::Sandbox)?;
        fs::write(&log, bytes).map_err(StandFault::io(&log))?;
        if is_profiled(member) {
            // `copy_out` takes an existing empty folder.
            let host = profile_dir(dir, &member.launch.service, member.launch.replica);
            fs::create_dir_all(&host).map_err(StandFault::io(&host))?;
            sandbox
                .copy_out(&member.id, PROFILE, &host)
                .map_err(StandFault::Sandbox)?;
        }
        sandbox.destroy(&member.id).map_err(StandFault::Sandbox)?;
        fleet.members.remove(0);
    }
    Ok(())
}

/// After a stand fault or `Cancel`: the logs it can still read, then `destroy` of every member;
/// errors are dropped, the first one already ends the run.
pub(crate) fn teardown(sandbox: &impl Sandbox, fleet: &mut Fleet, dir: &Utf8Path) {
    for member in fleet.members.drain(..) {
        if let Ok(bytes) = sandbox.logs(&member.id) {
            let _ = fs::write(log_path(dir, &member), bytes);
        }
        let _ = sandbox.destroy(&member.id);
    }
}

/// The CPU samples of a profiled instance of a measured repeat, after its `destroy`.
pub(crate) fn samples(
    dir: &Utf8Path,
    service: &str,
    replica: u32,
) -> Result<ProfileOutcome, StandFault> {
    let paths = PathMap::new(spec::TREE);
    match profile::read(&profile_dir(dir, service, replica), &paths) {
        Ok(samples) => Ok(ProfileOutcome::Read(samples)),
        Err(ProfileError::NoProfile) => Ok(ProfileOutcome::NoProfile),
        Err(error) => Err(StandFault::Profile(error)),
    }
}

/// The calls of a profiled instance of the coverage repeat; a replica without chunks adds none.
pub(crate) fn coverage(
    dir: &Utf8Path,
    service: &str,
    replica: u32,
) -> Result<Coverage, StandFault> {
    let paths = PathMap::new(spec::TREE);
    match profile::read_coverage(&profile_dir(dir, service, replica), &paths) {
        Ok(coverage) => Ok(coverage),
        Err(ProfileError::NoProfile) => Ok(Coverage::default()),
        Err(error) => Err(StandFault::Profile(error)),
    }
}
