//! `Stand` into the `Launch` of each service and replica of a phase. A service mounts the same
//! guest paths in setup, in every repeat and in `inspect`, only the host folders differ: a restore
//! needs every bind path the snapshot's VM had (plan, row 87).

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use camino::Utf8Path;
use profile::{ProfileMode, node_env};
use sandbox::{Exposed, ImageRef, Launch, Mount, Outbound, Peer, Seed, SnapshotId, VolumeName};
use spec::{Profiler, Service, ServiceKind, ServiceName, Stand};

use crate::ready::{Health, Start};

const OUT: &str = "/stand/out";
const PRELOAD: &str = "/stand/preload";
pub(crate) const PROFILE: &str = "/stand/profile";

/// What the VMs of a version start from once its setup passed.
#[derive(Debug, Default)]
pub(crate) struct Seeds {
    /// The snapshot of the prepare service after it exited 0: the version snapshot.
    pub(crate) version: Option<SnapshotId>,
    /// The setup snapshot of each service with a volume that runs in setup.
    pub(crate) volumes: BTreeMap<ServiceName, SnapshotId>,
}

impl Seeds {
    pub(crate) fn snapshots(&self) -> impl Iterator<Item = &SnapshotId> {
        self.version.iter().chain(self.volumes.values())
    }
}

/// What differs between the phases of a version.
pub(crate) struct Frame<'a> {
    /// The host folder of `/stand/out` for the workload and every service with a probe.
    pub(crate) out: &'a Utf8Path,
    pub(crate) tar: &'a Utf8Path,
    pub(crate) seeds: &'a Seeds,
    /// `None` in setup: only the repeats are measured.
    pub(crate) mode: Option<ProfileMode>,
    /// The host folder of `/stand/preload`.
    pub(crate) preload: &'a Utf8Path,
    pub(crate) span: Duration,
}

/// The services of setup: the setup services and all they wait for, directly or through others.
pub(crate) fn in_setup(stand: &Stand) -> BTreeSet<&ServiceName> {
    let mut found = BTreeSet::new();
    let mut next: Vec<&ServiceName> = stand
        .services
        .iter()
        .filter(|(_, service)| service.kind == ServiceKind::Setup)
        .map(|(name, _)| name)
        .collect();
    while let Some(name) = next.pop() {
        if found.insert(name) {
            next.extend(stand.services[name].depends_on.keys());
        }
    }
    found
}

/// The levels of `stand` with only the services `keep` takes; empty levels left out.
pub(crate) fn levels(
    stand: &Stand,
    keep: impl Fn(&ServiceName, &Service) -> bool,
) -> Vec<Vec<&ServiceName>> {
    stand
        .levels
        .iter()
        .map(|level| {
            level
                .iter()
                .filter(|name| keep(name, &stand.services[*name]))
                .collect::<Vec<_>>()
        })
        .filter(|level| !level.is_empty())
        .collect()
}

/// A profiler only where a repeat runs the service.
fn profiler(service: &Service) -> Option<Profiler> {
    service
        .profile
        .filter(|_| service.kind != ServiceKind::Setup)
}

pub(crate) fn any_profiled(stand: &Stand) -> bool {
    stand
        .services
        .values()
        .any(|service| profiler(service).is_some())
}

/// Every replica of every service of `level`.
pub(crate) fn starts(stand: &Stand, level: &[&ServiceName], frame: &Frame) -> Vec<Start> {
    let mut starts = Vec::new();
    for name in level {
        let service = &stand.services[*name];
        for replica in 1..=service.replicas.get() {
            starts.push(Start {
                kind: service.kind,
                launch: of(stand, name, replica, frame),
                health: service.healthcheck.as_ref().map(Health::from),
            });
        }
    }
    starts
}

pub(crate) fn of(stand: &Stand, name: &ServiceName, replica: u32, frame: &Frame) -> Launch {
    let service = &stand.services[name];
    let mut env = service.environment.clone();
    let mut mounts = Vec::new();
    for mount in &service.mounts {
        match mount {
            spec::Mount::Tree => {}
            spec::Mount::Harness {
                source,
                target,
                read_only,
            } => mounts.push(Mount::Bind {
                host: source.clone(),
                guest: target.clone(),
                read_only: *read_only,
            }),
            // `spec` refuses `:ro` on a volume.
            spec::Mount::Volume { name, target, .. } => mounts.push(Mount::Volume {
                name: VolumeName::new(name),
                guest: target.clone(),
            }),
        }
    }
    if service.kind == ServiceKind::Workload || probed(stand, name) {
        mounts.push(Mount::Bind {
            host: frame.out.to_owned(),
            guest: OUT.to_owned(),
            read_only: false,
        });
    }
    if let (Some(mode), Some(Profiler::Node)) = (frame.mode, profiler(service)) {
        mounts.push(Mount::Bind {
            host: frame.preload.to_owned(),
            guest: PRELOAD.to_owned(),
            read_only: true,
        });
        mounts.push(Mount::Scratch {
            guest: PROFILE.to_owned(),
        });
        let inherited = env.get("NODE_OPTIONS").cloned();
        let vars = node_env(PRELOAD, PROFILE, inherited.as_deref(), mode).into_vars();
        env.extend(vars.map(|(key, value)| (key.to_owned(), value)));
    }
    Launch {
        service: name.as_str().to_owned(),
        replica,
        image: ImageRef::new(&service.image),
        cpus: service.cpus.get().div_ceil(1000),
        memory: service.memory.get(),
        env,
        workdir: service.working_dir.clone(),
        entrypoint: service.entrypoint.clone(),
        command: service.command.clone(),
        mounts,
        ports: service
            .expose
            .iter()
            .map(|port| Exposed(port.get()))
            .collect(),
        peers: peers(stand),
        outbound: match service.outbound {
            spec::Outbound::Off => Outbound::Off,
            spec::Outbound::Public => Outbound::Public,
        },
        stop_signal: service.stop_signal.clone(),
        stop_grace: service.stop_grace,
        max_duration: frame.span,
        seed: seed(name, service, frame),
    }
}

/// A service on the tree starts from the version snapshot. Before there is one it unpacks the
/// tree: the prepare service, which every other service on the tree waits for, or each of them
/// when there is no prepare service. A service with a volume starts from its setup snapshot once
/// setup took one.
fn seed(name: &ServiceName, service: &Service, frame: &Frame) -> Option<Seed> {
    if service
        .mounts
        .iter()
        .any(|mount| matches!(mount, spec::Mount::Tree))
    {
        let fresh = || Seed::Tree {
            tar: frame.tar.to_owned(),
            at: spec::TREE.to_owned(),
        };
        return Some(
            frame
                .seeds
                .version
                .clone()
                .map_or_else(fresh, Seed::Snapshot),
        );
    }
    frame.seeds.volumes.get(name).cloned().map(Seed::Snapshot)
}

fn probed(stand: &Stand, name: &ServiceName) -> bool {
    stand.probes.values().any(|probe| probe.service == *name)
}

/// Every service of the stand, also those that do not run in this phase: their names resolve and
/// refuse, as a stopped container's would.
fn peers(stand: &Stand) -> Vec<Peer> {
    stand
        .services
        .iter()
        .map(|(name, service)| Peer {
            service: name.as_str().to_owned(),
            replicas: service.replicas.get(),
            ports: service.expose.iter().map(|port| port.get()).collect(),
        })
        .collect()
}
