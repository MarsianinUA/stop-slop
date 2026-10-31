//! The end-of-run snapshots (architecture plan, row 64): `kept.json` in the run folder is their
//! only list, and `inspect` starts one of them again in a VM without peers and network.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use sandbox::{
    ExecOutcome, ImageRef, Launch, Mount, Outbound, Sandbox, Seed, SnapshotId, VolumeName, Waitable,
};
use serde::{Deserialize, Serialize};
use spec::{Healthcheck, ServiceKind};

use crate::drain;
use crate::launch::OUT;
use crate::ready::{self, Fleet, Halt, Start};
use crate::{Cancel, InspectError, Inspected, Side, StandFault};

const KEPT: &str = "kept.json";
/// The readiness of the inspected service, and then its `argv`, each.
const INSPECT: Duration = Duration::from_secs(60);

/// A kept end-of-run snapshot and what `inspect` needs to start the service again.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Kept {
    pub side: Side,
    pub service: String,
    pub snapshot: SnapshotId,
    pub launch: KeptLaunch,
}

/// Image, limits, env, workdir, entrypoint, command, every mount of the kept repeat and the
/// healthcheck; a service with a volume never mounts `${STAND_REPO}`, so the snapshot is its only
/// seed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeptLaunch {
    image: String,
    cpus: u32,
    memory: u64,
    env: BTreeMap<String, String>,
    workdir: Option<String>,
    entrypoint: Option<Vec<String>>,
    command: Option<Vec<String>>,
    mounts: Vec<KeptMount>,
    stop_signal: Option<String>,
    stop_grace_ms: u64,
    max_duration_ms: u64,
    health: Option<KeptHealth>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
enum KeptMount {
    Bind {
        host: Utf8PathBuf,
        guest: String,
        read_only: bool,
    },
    Volume {
        name: String,
        guest: String,
    },
    Scratch {
        guest: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeptHealth {
    test: Vec<String>,
    interval_ms: u64,
    timeout_ms: u64,
    retries: u32,
    start_period_ms: u64,
}

impl KeptLaunch {
    fn of(launch: &Launch, health: Option<&Healthcheck>) -> KeptLaunch {
        let mounts = launch.mounts.iter().map(|mount| match mount {
            Mount::Bind {
                host,
                guest,
                read_only,
            } => KeptMount::Bind {
                host: host.clone(),
                guest: guest.clone(),
                read_only: *read_only,
            },
            Mount::Volume { name, guest } => KeptMount::Volume {
                name: name.to_string(),
                guest: guest.clone(),
            },
            Mount::Scratch { guest } => KeptMount::Scratch {
                guest: guest.clone(),
            },
        });
        KeptLaunch {
            image: launch.image.to_string(),
            cpus: launch.cpus,
            memory: launch.memory,
            env: launch.env.clone(),
            workdir: launch.workdir.clone(),
            entrypoint: launch.entrypoint.clone(),
            command: launch.command.clone(),
            mounts: mounts.collect(),
            stop_signal: launch.stop_signal.clone(),
            stop_grace_ms: millis(launch.stop_grace),
            max_duration_ms: millis(launch.max_duration),
            health: health.map(|health| KeptHealth {
                test: health.test.clone(),
                interval_ms: millis(health.interval),
                timeout_ms: millis(health.timeout),
                retries: health.retries,
                start_period_ms: millis(health.start_period),
            }),
        }
    }

    /// The kept snapshot with every mount of the kept repeat, since a restore needs each bind path
    /// (plan, row 87), and `/stand/out` on `out`; no peers, no published ports, no network.
    fn restore(&self, service: &str, snapshot: &SnapshotId, out: &Utf8Path) -> Launch {
        let mounts = self.mounts.iter().filter_map(|mount| match mount {
            KeptMount::Bind { guest, .. } if guest == OUT => None,
            KeptMount::Bind {
                host,
                guest,
                read_only,
            } => Some(Mount::Bind {
                host: host.clone(),
                guest: guest.clone(),
                read_only: *read_only,
            }),
            KeptMount::Volume { name, guest } => Some(Mount::Volume {
                name: VolumeName::new(name),
                guest: guest.clone(),
            }),
            KeptMount::Scratch { guest } => Some(Mount::Scratch {
                guest: guest.clone(),
            }),
        });
        let mut mounts: Vec<Mount> = mounts.collect();
        // A service without a probe had none; the stdout of `argv` goes there.
        mounts.push(Mount::Bind {
            host: out.to_owned(),
            guest: OUT.to_owned(),
            read_only: false,
        });
        Launch {
            service: service.to_owned(),
            replica: 1,
            image: ImageRef::new(&self.image),
            cpus: self.cpus,
            memory: self.memory,
            env: self.env.clone(),
            workdir: self.workdir.clone(),
            entrypoint: self.entrypoint.clone(),
            command: self.command.clone(),
            mounts,
            ports: Vec::new(),
            peers: Vec::new(),
            outbound: Outbound::Off,
            stop_signal: self.stop_signal.clone(),
            stop_grace: Duration::from_millis(self.stop_grace_ms),
            max_duration: Duration::from_millis(self.max_duration_ms),
            seed: Some(Seed::Snapshot(snapshot.clone())),
        }
    }

    fn health(&self) -> Option<Healthcheck> {
        self.health.as_ref().map(|health| Healthcheck {
            test: health.test.clone(),
            interval: Duration::from_millis(health.interval_ms),
            timeout: Duration::from_millis(health.timeout_ms),
            retries: health.retries,
            start_period: Duration::from_millis(health.start_period_ms),
        })
    }

    /// The first bind folder that is gone: a harness folder of a sealed copy `clean` removed, or
    /// the preload of a run folder half gone.
    fn gone(&self) -> Option<&Utf8Path> {
        self.mounts.iter().find_map(|mount| match mount {
            KeptMount::Bind { host, guest, .. } if guest != OUT && !host.is_dir() => {
                Some(host.as_path())
            }
            _ => None,
        })
    }
}

/// The snapshots `run_dir/kept.json` lists, the only list of kept snapshots; no file is an empty
/// list.
pub fn kept_ids(run_dir: &Utf8Path) -> Result<Vec<SnapshotId>, StandFault> {
    let kept = read(run_dir)?;
    Ok(kept.into_iter().map(|kept| kept.snapshot).collect())
}

/// The snapshot of each member with a volume of a repeat that just stopped.
pub(crate) fn take(
    sandbox: &impl Sandbox,
    fleet: &Fleet,
    side: Side,
    kept: &mut Vec<Kept>,
) -> Result<(), StandFault> {
    let volumes = fleet
        .members
        .iter()
        .filter(|member| drain::has_volume(member));
    for member in volumes {
        let (_, snapshot) = drain::snapshot(sandbox, &member.id).map_err(StandFault::Sandbox)?;
        kept.push(Kept {
            side,
            service: member.launch.service.clone(),
            snapshot,
            launch: KeptLaunch::of(&member.launch, member.health.as_ref()),
        });
    }
    Ok(())
}

/// Nothing when nothing was kept: the kept run of the session stays the one before (`store`).
pub(crate) fn write(run_dir: &Utf8Path, kept: &[Kept]) -> Result<(), StandFault> {
    if kept.is_empty() {
        return Ok(());
    }
    crate::write_whole(&run_dir.join(KEPT), &kept)
}

fn read(run_dir: &Utf8Path) -> Result<Vec<Kept>, StandFault> {
    Ok(crate::read_json(&run_dir.join(KEPT))?.unwrap_or_default())
}

/// Restores the kept snapshot of `service` on `side` from `run_dir/kept.json` into a VM without
/// peers and network, runs `argv` under the inspect deadline with stdout to
/// `run_dir/inspect/<n>.out`, destroys the VM. Nothing reaches the snapshot.
pub fn inspect(
    sandbox: &impl Sandbox,
    run_dir: &Utf8Path,
    side: Side,
    service: &str,
    argv: &[String],
    cancel: &Cancel,
) -> Result<Inspected, InspectError> {
    let kept = read(run_dir).map_err(InspectError::Stand)?;
    let Some(entry) = kept
        .iter()
        .find(|kept| kept.side == side && kept.service == service)
    else {
        return Err(InspectError::NotKept(format!(
            "{run_dir} keeps no snapshot of `{service}` on the {side} side"
        )));
    };
    if let Some(gone) = entry.launch.gone() {
        return Err(InspectError::NotStarted {
            service: service.to_owned(),
            cause: format!("its folder {gone} is gone"),
        });
    }
    if cancel.is_cancelled() {
        return Err(InspectError::Cancelled);
    }
    let dir = run_dir.join("inspect");
    fs::create_dir_all(&dir)
        .map_err(StandFault::io(&dir))
        .map_err(InspectError::Stand)?;
    let out = free(&dir)?;
    let start = Start {
        kind: ServiceKind::LongRunning,
        launch: entry.launch.restore(service, &entry.snapshot, &dir),
        health: entry.launch.health(),
    };
    let mut fleet = Fleet::default();
    fleet
        .start(sandbox, vec![start])
        .map_err(|halt| refused(halt, service, None))?;
    let looked = look(sandbox, &mut fleet, argv, &out, cancel);
    let member = &fleet.members[0];
    let destroyed = sandbox.destroy(&member.id);
    let code = member.exit.and_then(|(code, _)| code);
    let outcome = looked.map_err(|halt| refused(halt, service, code))?;
    destroyed.map_err(|error| InspectError::Stand(StandFault::Sandbox(error)))?;
    Ok(Inspected { outcome, out })
}

fn look(
    sandbox: &impl Sandbox,
    fleet: &mut Fleet,
    argv: &[String],
    out: &Utf8Path,
    cancel: &Cancel,
) -> Result<ExecOutcome, Halt> {
    let deadline = sandbox.now() + INSPECT;
    let id = fleet.members[0].id.clone();
    if fleet.members[0].health.is_some() {
        fleet.healthy(sandbox, 0, deadline, cancel)?;
    } else if let Some(exited) = sandbox.wait(&[Waitable::Sandbox(id.clone())], sandbox.now())? {
        // `create` returns once the command runs; one look shows a command that ended at once.
        fleet.members[0].exit = Some((exited.code, exited.at));
        return Err(Halt::Down(0));
    }
    ready::stop_if(cancel)?;
    Ok(sandbox.exec(&id, argv, sandbox.now() + INSPECT, out)?)
}

fn refused(halt: Halt, service: &str, code: Option<i32>) -> InspectError {
    let cause = match halt {
        Halt::Down(_) => match code {
            Some(code) => format!("it exited with {code}"),
            None => "it was killed".to_owned(),
        },
        Halt::Unhealthy(_) => "it stayed unhealthy".to_owned(),
        Halt::Deadline(_) => format!("it was not healthy within {} s", INSPECT.as_secs()),
        Halt::Cancelled => return InspectError::Cancelled,
        Halt::Port(error) => return InspectError::Stand(StandFault::Sandbox(error)),
    };
    InspectError::NotStarted {
        service: service.to_owned(),
        cause: format!("{cause}, without its peers and network"),
    }
}

/// `<dir>/<n>.out` for the first `n` from 1 that nothing in `dir` is named: the outputs of earlier
/// looks keep theirs, and a link the guest left there is never handed out.
fn free(dir: &Utf8Path) -> Result<Utf8PathBuf, InspectError> {
    let mut n = 1;
    loop {
        let out = dir.join(format!("{n}.out"));
        match fs::symlink_metadata(&out) {
            Ok(_) => n += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(out),
            Err(error) => return Err(InspectError::Stand(StandFault::io(&out)(error))),
        }
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
