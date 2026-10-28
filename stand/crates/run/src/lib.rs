//! One atomic stand run in sandboxes, and a look into its kept snapshots. `run` knows no type of
//! `store` or `source`: `cli` gives it the tar files of the trees, the cache entries and an empty
//! run folder, and gets back `report::Raw`.

use std::fmt;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use profile::ProfileError;
use report::{Failure, Phase, Raw};
use sandbox::{ExecOutcome, Sandbox, SandboxError};
use serde::{Deserialize, Serialize};
use spec::Stand;

mod cache;
mod drain;
mod keep;
mod lane;
mod launch;
mod probes;
mod ready;

pub use cache::cached_ids;
pub use keep::{Kept, KeptLaunch, inspect, kept_ids};

/// Over the sum of the timeouts in `span`: readiness, stops, drains and snapshots between them.
const SPAN_MARGIN: Duration = Duration::from_secs(10 * 60);

/// `span` on the defaults of `spec` (setup 10m, timeout 5m, `repeat` 3, no extras), for commands
/// that wait for the lock without a readable stand file.
pub const DEFAULT_SPAN: Duration = Duration::from_secs(60 * 60);

pub fn run(
    stand: &Stand,
    sandbox: &impl Sandbox,
    versions: &Versions,
    run_dir: &Utf8Path,
    cancel: &Cancel,
) -> Result<Raw, RunError> {
    lane::run(stand, sandbox, versions, run_dir, cancel)
}

/// `max_duration` of every VM and the lock-wait limit of `cli`: a whole run with `--before`, the
/// coverage, fault and second-pass repeats included, plus the margin.
pub fn span(stand: &Stand) -> Duration {
    // In halves of `timeout`, over both versions: two per measured repeat, three for the coverage
    // repeat of the head (x 1.5), eight for the two fault repeats, eight for the second pass (x 2).
    let mut halves = stand.repeat.get().saturating_mul(4);
    if launch::any_profiled(stand) {
        halves += 3;
    }
    if stand.fault.is_some() {
        halves += 8;
    }
    if stand.second_pass.is_some() {
        halves += 8;
    }
    stand
        .setup_timeout
        .get()
        .saturating_mul(2)
        .saturating_add(stand.timeout.get().saturating_mul(halves) / 2)
        .saturating_add(SPAN_MARGIN)
}

#[derive(Debug)]
pub struct Versions {
    pub head: Version,
    pub before: Option<Version>,
}

#[derive(Debug)]
pub struct Version {
    /// The tar file of the version's tree; every fresh version disk of that version unpacks it.
    pub tar: Utf8PathBuf,
    pub setup: SetupCache,
}

/// The setup cache entry of a version (plan, row 88): `run` lays out its folder, `store` creates
/// and deletes it.
#[derive(Debug)]
pub enum SetupCache {
    /// A passed setup of the same key: no setup VM starts, the repeats start from its snapshots,
    /// and its outputs are copied into `setup-<side>/out/`.
    Hit(Utf8PathBuf),
    /// An empty folder: setup runs as without a cache; once the whole setup passed,
    /// `setup-<side>/out/` is copied into its `out/` and `entry.json` is written.
    Build(Utf8PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Side {
    Head,
    Before,
}

/// The word in the names of the run folder: `setup-head`, `repeat-2-before`.
impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Side::Head => "head",
            Side::Before => "before",
        })
    }
}

#[derive(Debug)]
pub struct Inspected {
    pub outcome: ExecOutcome,
    /// The stdout of `argv`, `<run>/inspect/<n>.out`; its stderr is `<n>.out.err` beside it.
    pub out: Utf8PathBuf,
}

/// Ctrl+C sets it; `run` and the lock wait check it.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// `phase` is the step the run was in, for `failure.phase` of the line on exit 3 and 130: `Setup`
/// in the setup of a version and before the first one, `Ready` in a repeat before the batch with
/// the workload is created, `Workload` from it to the end of the repeat and at the end of the run.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("the app failed")]
    App(Box<Failed>),
    #[error("{fault}")]
    Stand { phase: Phase, fault: StandFault },
    #[error("cancelled")]
    Cancelled { phase: Phase },
}

/// What the head collected before it failed, the failed repeat included. `kept`: the head
/// services whose failure snapshot `kept.json` lists; `cli` turns each into `Next::Inspect`.
/// Empty when setup failed.
#[derive(Debug)]
pub struct Failed {
    pub failure: Failure,
    pub raw: Raw,
    pub kept: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum StandFault {
    #[error(transparent)]
    Sandbox(SandboxError),
    #[error(transparent)]
    Profile(ProfileError),
    #[error("{path}: {source}")]
    Io {
        path: Utf8PathBuf,
        #[source]
        source: io::Error,
    },
}

impl StandFault {
    fn io(path: &Utf8Path) -> impl FnOnce(io::Error) -> StandFault + '_ {
        move |source| StandFault::Io {
            path: path.to_owned(),
            source,
        }
    }

    /// A file of `run` that is missing or does not read.
    fn bad(path: &Utf8Path, message: impl fmt::Display) -> StandFault {
        StandFault::Io {
            path: path.to_owned(),
            source: io::Error::new(io::ErrorKind::InvalidData, message.to_string()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InspectError {
    /// No such run folder, side, service or kept snapshot.
    #[error("{0}")]
    NotKept(String),
    /// Its harness folder or the preload of the run is gone (no VM is created), or it exited or
    /// never got healthy: `inspect` gives it no peers and no network. Exit 2.
    #[error("{service} did not start in inspect: {cause}")]
    NotStarted { service: String, cause: String },
    #[error(transparent)]
    Stand(StandFault),
    #[error("cancelled")]
    Cancelled,
}
