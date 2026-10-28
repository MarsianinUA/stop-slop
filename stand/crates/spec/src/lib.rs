use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::num::NonZeroU32;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};

use crate::tree::Field;

mod allow;
mod bind;
mod graph;
mod tree;
mod units;

/// The tree of the measured version in a guest; its version disk mounts at the parent, `/w`.
pub const TREE: &str = "/w/app";

pub fn load(path: &Utf8Path) -> Result<Stand, SpecError> {
    let file = camino::absolute_utf8(path).map_err(|source| SpecError::Io {
        file: path.to_owned(),
        source,
    })?;
    let text = match fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(SpecError::Missing(file));
        }
        Err(source) => return Err(SpecError::Io { file, source }),
    };
    read(&text, &file).map_err(|invalid| SpecError::Invalid {
        file: file.clone(),
        line: invalid.line,
        key: invalid.key,
        message: invalid.message,
    })
}

fn read(text: &str, file: &Utf8Path) -> Result<Stand, Invalid> {
    let node = tree::parse(text)?;
    let root = Field::root(&node);
    allow::check(&root)?;
    let dir = file.parent().expect("a file that was read is in a folder");
    let mut bound = bind::stand(&root, dir)?;
    let graph = graph::of(&root, &mut bound.services, &bound.workload)?;
    Ok(Stand {
        file: file.to_owned(),
        services: bound.services,
        levels: graph.levels,
        workload: bound.workload,
        prepare: graph.prepare,
        repeat: bound.repeat,
        setup_timeout: bound.setup_timeout,
        timeout: bound.timeout,
    })
}

#[derive(Debug)]
pub struct Stand {
    /// Absolute.
    pub file: Utf8PathBuf,
    pub services: BTreeMap<ServiceName, Service>,
    /// Every service, each after all it depends on.
    pub levels: Vec<Vec<ServiceName>>,
    pub workload: ServiceName,
    /// The setup service with `${STAND_REPO}` that every other service with `${STAND_REPO}`
    /// depends on; its snapshot after it exits seeds them, so they all have its `image` and it
    /// binds nothing else. `None`: no setup service mounts `${STAND_REPO}`.
    pub prepare: Option<ServiceName>,
    pub repeat: Repeat,
    pub setup_timeout: Timeout,
    pub timeout: Timeout,
}

impl Stand {
    /// Every service and replica at once: whole vCPUs (`ceil(cpus)` per instance) and memory
    /// limits.
    pub fn demand(&self) -> Demand {
        self.services
            .values()
            .fold(Demand { cpus: 0, memory: 0 }, |demand, service| {
                let replicas = service.replicas.get();
                Demand {
                    cpus: demand
                        .cpus
                        .saturating_add(service.cpus.get().div_ceil(1000).saturating_mul(replicas)),
                    memory: demand
                        .memory
                        .saturating_add(service.memory.get().saturating_mul(u64::from(replicas))),
                }
            })
    }
}

/// `cli` copies the two numbers into `budget::Demand`: `budget` does not depend on `spec`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Demand {
    pub cpus: u32,
    pub memory: u64,
}

#[derive(Debug)]
pub struct Service {
    pub kind: ServiceKind,
    pub image: String,
    pub entrypoint: Option<Vec<String>>,
    pub command: Option<Vec<String>>,
    pub environment: BTreeMap<String, String>,
    pub working_dir: Option<String>,
    pub mounts: Vec<Mount>,
    pub expose: Vec<Exposed>,
    pub depends_on: BTreeMap<ServiceName, Condition>,
    pub healthcheck: Option<Healthcheck>,
    /// `SIGTERM` unless `stop_signal` names another.
    pub stop_signal: String,
    /// 10s unless `stop_grace_period` says otherwise.
    pub stop_grace: Duration,
    pub cpus: Cpus,
    pub memory: Bytes,
    pub replicas: Replicas,
    pub profile: Option<Profiler>,
    pub outbound: Outbound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    LongRunning,
    Setup,
    Workload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Condition {
    Started,
    Healthy,
    CompletedSuccessfully,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outbound {
    Off,
    Public,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profiler {
    Node,
}

/// `CMD-SHELL` and a string `test` become `["sh", "-c", <text>]`.
#[derive(Debug)]
pub struct Healthcheck {
    pub test: Vec<String>,
    pub interval: Duration,
    pub timeout: Duration,
    pub retries: u32,
    pub start_period: Duration,
}

#[derive(Debug)]
pub enum Mount {
    /// `${STAND_REPO}:/w/app`: the measured version's tree on the service's own version disk.
    Tree,
    /// A relative source, absolute here, from the folder of the file: the same for both versions.
    Harness {
        source: Utf8PathBuf,
        target: String,
        read_only: bool,
    },
    /// A name of the top-level `volumes`; one service with one replica mounts it.
    Volume {
        name: String,
        target: String,
        read_only: bool,
    },
}

/// `[a-zA-Z0-9][a-zA-Z0-9_-]*`, and not `stand` or `volume`, which measure names reserve
/// (`stand.wall`, `volume:pgdata.bytes`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServiceName(String);

impl ServiceName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exposed(u16);

impl Exposed {
    pub fn get(self) -> u16 {
        self.0
    }
}

/// Thousandths of a CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cpus(u32);

impl Cpus {
    pub fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bytes(u64);

impl Bytes {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeout(Duration);

impl Timeout {
    pub fn get(self) -> Duration {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Repeat(NonZeroU32);

impl Repeat {
    pub fn get(self) -> u32 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Replicas(NonZeroU32);

impl Replicas {
    pub fn get(self) -> u32 {
        self.0.get()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SpecError {
    #[error("no stand file at {0}")]
    Missing(Utf8PathBuf),
    /// `key` such as `services.api.deploy.resources.limits.pids`; `line` from 1.
    #[error("{file}:{line}: {key}: {message}")]
    Invalid {
        file: Utf8PathBuf,
        line: u32,
        key: String,
        message: String,
    },
    #[error("{file}: {source}")]
    Io {
        file: Utf8PathBuf,
        #[source]
        source: io::Error,
    },
}

/// A `SpecError::Invalid` before `load` puts in the file.
#[derive(Debug)]
struct Invalid {
    line: u32,
    key: String,
    message: String,
}
