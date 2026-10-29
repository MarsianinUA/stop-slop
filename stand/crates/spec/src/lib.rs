use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::num::NonZeroU32;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};

use crate::tree::Field;

mod allow;
mod bind;
mod graph;
mod recipe;
mod tree;
mod units;
mod xstand;

pub use recipe::Recipe;
pub use xstand::{Change, ChangeError};

/// The tree of the measured version in a guest; its version disk mounts at the parent, `/w`.
pub const TREE: &str = "/w/app";

/// The `stop_signal` names `spec` takes: Linux's, without the real-time ones.
pub const SIGNALS: &[&str] = &[
    "SIGHUP",
    "SIGINT",
    "SIGQUIT",
    "SIGILL",
    "SIGTRAP",
    "SIGABRT",
    "SIGIOT",
    "SIGBUS",
    "SIGFPE",
    "SIGKILL",
    "SIGUSR1",
    "SIGSEGV",
    "SIGUSR2",
    "SIGPIPE",
    "SIGALRM",
    "SIGTERM",
    "SIGSTKFLT",
    "SIGCHLD",
    "SIGCONT",
    "SIGSTOP",
    "SIGTSTP",
    "SIGTTIN",
    "SIGTTOU",
    "SIGURG",
    "SIGXCPU",
    "SIGXFSZ",
    "SIGVTALRM",
    "SIGPROF",
    "SIGWINCH",
    "SIGIO",
    "SIGPOLL",
    "SIGPWR",
    "SIGSYS",
];

/// `max_tolerance` when `stand.toml` has none.
const MAX_TOLERANCE: u16 = 25;

pub fn load(path: &Utf8Path, policy: &Policy) -> Result<Stand, SpecError> {
    let file = camino::absolute_utf8(path).map_err(SpecError::io(path))?;
    let text = match fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(SpecError::Missing(file));
        }
        Err(source) => return Err(SpecError::Io { file, source }),
    };
    let recipe = recipe::of(folder(&file))?;
    read(&text, &file, recipe, policy).map_err(|invalid| SpecError::Invalid {
        file: file.clone(),
        line: invalid.line,
        key: invalid.key,
        message: invalid.message,
    })
}

fn read(text: &str, file: &Utf8Path, recipe: Recipe, policy: &Policy) -> Result<Stand, Invalid> {
    let node = tree::parse(text)?;
    let root = Field::root(&node);
    allow::check(&root)?;
    let mut bound = bind::stand(&root, folder(file))?;
    let graph = graph::of(&root, &mut bound.services, &bound.workload)?;
    let probes = xstand::probes(&root, &bound.services)?;
    let expect = xstand::expect(&root, policy)?;
    let fault = xstand::fault(&root, &bound.services)?;
    let second_pass = xstand::second_pass(&root, &probes)?;
    Ok(Stand {
        file: file.to_owned(),
        recipe,
        services: bound.services,
        levels: graph.levels,
        workload: bound.workload,
        prepare: graph.prepare,
        repeat: bound.repeat,
        setup_timeout: bound.setup_timeout,
        timeout: bound.timeout,
        probes,
        expect,
        fault,
        second_pass,
    })
}

/// The folder of the recipe, which relative bind sources start from.
fn folder(file: &Utf8Path) -> &Utf8Path {
    file.parent().expect("a file that was read is in a folder")
}

#[derive(Debug, Clone, Copy)]
pub struct Policy {
    pub max_tolerance: Percent,
}

impl Policy {
    /// `None` is 25; outside 1..=1000 is a `PolicyError`, which `cli` reports as the `ConfigError`
    /// of `max_tolerance`.
    pub fn new(max_tolerance: Option<u16>) -> Result<Policy, PolicyError> {
        let max_tolerance = max_tolerance.unwrap_or(MAX_TOLERANCE);
        units::percent(max_tolerance)
            .map(|max_tolerance| Policy { max_tolerance })
            .ok_or(PolicyError(max_tolerance))
    }
}

#[derive(Debug)]
pub struct Stand {
    /// Absolute.
    pub file: Utf8PathBuf,
    /// Every file of the folder of `file`: the stand file, the harness and anything else there.
    pub recipe: Recipe,
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
    pub probes: BTreeMap<ProbeName, Probe>,
    pub expect: Vec<Cost>,
    pub fault: Option<Fault>,
    pub second_pass: Option<SecondPass>,
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

    /// The services that run in setup: the setup services and all they wait for, directly or
    /// through others. `after-setup` probes run only in these.
    pub fn in_setup(&self) -> BTreeSet<&ServiceName> {
        graph::in_setup(&self.services)
    }
}

/// `cli` copies the two numbers into `budget::Demand`: `budget` does not depend on `spec`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Demand {
    pub cpus: u32,
    pub memory: u64,
}

#[derive(Debug)]
pub struct Probe {
    /// A long-running service; for `AfterSetup` one that runs in setup.
    pub service: ServiceName,
    pub at: Tap,
    pub run: Vec<String>,
    pub key: Option<Pointer>,
    pub ignore: Vec<Pointer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tap {
    AfterSetup,
    AfterWorkload,
    OnFailure,
}

/// `measure` is `<subject>.<metric>`; `report` checks it against its measures (`unknown-expect`).
#[derive(Debug, Clone)]
pub struct Cost {
    pub measure: String,
    pub change: Change,
    pub reason: String,
}

#[derive(Debug)]
pub struct Fault {
    pub target: Target,
    pub mode: FaultMode,
}

/// A service and one of its `expose` ports.
#[derive(Debug, Clone)]
pub struct Target {
    pub service: ServiceName,
    pub port: Exposed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultMode {
    Down,
    Slow(Duration),
}

/// `x-stand.second_pass: true`; `unchanged` comes from `x-stand.expect.unchanged`.
#[derive(Debug)]
pub struct SecondPass {
    pub unchanged: Vec<Unchanged>,
}

/// A file of `/stand/out` or `probe.<name>.jsonl`.
#[derive(Debug)]
pub struct Unchanged {
    pub file: String,
    pub reason: String,
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
    /// `None`: the image's `STOPSIGNAL`, else `SIGTERM`, as Docker does; the sandbox reads the image.
    pub stop_signal: Option<String>,
    /// 10s unless `stop_grace_period` says otherwise.
    pub stop_grace: Duration,
    pub cpus: Cpus,
    pub memory: Bytes,
    pub replicas: Replicas,
    pub profile: Option<Profiler>,
    pub outbound: Outbound,
}

impl Service {
    /// It mounts `${STAND_REPO}`.
    pub fn on_tree(&self) -> bool {
        self.mounts.iter().any(|mount| matches!(mount, Mount::Tree))
    }
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
#[derive(Debug, Clone)]
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
    /// Always `read_only`: a service that wrote there would change the recipe.
    Harness {
        source: Utf8PathBuf,
        target: String,
        read_only: bool,
    },
    /// A name of the top-level `volumes`; one service with one replica mounts it, never
    /// `read_only`, or the volume would stay empty.
    Volume {
        name: String,
        target: String,
        read_only: bool,
    },
}

/// `[a-zA-Z0-9][a-zA-Z0-9_-]*`, not `stand` or `volume`, which measure names reserve
/// (`stand.wall`, `volume:pgdata.bytes`), and not a device name of Windows (`aux`). No two
/// differ only in case: each names files on the host.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServiceName(String);

impl ServiceName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// As `ServiceName`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProbeName(String);

impl ProbeName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// RFC 6901: `""` or `/a/0/b`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pointer(String);

impl Pointer {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whole percents, 1..=1000.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Percent(u16);

impl Percent {
    pub fn get(self) -> u16 {
        self.0
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

impl SpecError {
    fn io(file: &Utf8Path) -> impl Fn(io::Error) -> SpecError {
        let file = file.to_owned();
        move |source| SpecError::Io {
            file: file.clone(),
            source,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0} is outside 1-1000")]
pub struct PolicyError(pub u16);

/// A `SpecError::Invalid` before `load` puts in the file.
#[derive(Debug)]
struct Invalid {
    line: u32,
    key: String,
    message: String,
}
