//! The port to the VMs of a stand run. `run` and `cli` see only the trait and these types: no
//! Compose types (`spec`) and no SDK. The adapters sit behind features: `fake` for tests,
//! `microsandbox` for real VMs.

use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

#[cfg(feature = "fake")]
mod fake;

#[cfg(feature = "fake")]
pub use fake::{Call, Fake, Rule, Scenario, Script};

// The module shares its name with the SDK crate: a bare `microsandbox::` path is ambiguous here,
// so the re-export goes through `self::`, and the module names the SDK `::microsandbox`.
#[cfg(feature = "microsandbox")]
mod microsandbox;

#[cfg(feature = "microsandbox")]
pub use self::microsandbox::Microsandbox;

pub trait Sandbox {
    /// Virtual in `Fake`: it moves only in `wait`.
    fn now(&self) -> Instant;
    fn pull(&self, images: &[ImageRef]) -> Result<Pulled, SandboxError>;
    /// One batch starts in parallel; ids in the order of `batch`. Returns once each service's command
    /// runs under `stand-guest run`: that is `service_started`. Env, workdir and command go into that exec,
    /// never onto the VM (a restored VM has no VM env), and every later exec of the VM gets the same env.
    fn create(&self, batch: &[Launch]) -> Result<Vec<SandboxId>, SandboxError>;
    /// Healthcheck attempts: `run` waits on them together with every sandbox of the batch and `Cancel`.
    fn exec_detached(&self, id: &SandboxId, argv: &[String]) -> Result<ExecId, SandboxError>;
    /// The first of `what` to exit, or `None` at `deadline`.
    fn wait(&self, what: &[Waitable], deadline: Instant) -> Result<Option<Exited>, SandboxError>;
    /// Probes and `inspect`: killed at `deadline`; its sockets reach only the guest's own loopback;
    /// stdout goes to `stdout` on the host, stderr to `<stdout>.err`.
    fn exec(
        &self,
        id: &SandboxId,
        argv: &[String],
        deadline: Instant,
        stdout: &Utf8Path,
    ) -> Result<ExecOutcome, SandboxError>;
    /// The service's `stop_signal`, then `stop_grace`, then a kill; the VM stays for `snapshot`, `logs`, `metrics`.
    fn stop(&self, id: &SandboxId) -> Result<Stopped, SandboxError>;
    /// From now until `destroy`, in the relays of `ids`.
    fn fault(&self, ids: &[SandboxId], fault: &Fault) -> Result<(), SandboxError>;
    // `metrics`, `logs`, `copy_out` work only before `destroy`; after it they are `Gone`.
    fn metrics(&self, id: &SandboxId) -> Result<Metrics, SandboxError>;
    fn logs(&self, id: &SandboxId) -> Result<Vec<u8>, SandboxError>;
    /// The guest folder as one tar packed in the guest, one SDK copy, unpacked into the empty folder `host`.
    fn copy_out(&self, id: &SandboxId, guest: &str, host: &Utf8Path) -> Result<(), SandboxError>;
    /// Used bytes of each volume of `id`, by `du -sk` in the guest; after `stop`, before `snapshot`.
    fn volume_sizes(&self, id: &SandboxId) -> Result<Vec<(VolumeName, u64)>, SandboxError>;
    /// After `stop`, or the exit of the prepare service, before `destroy`. FITRIM on every owned disk first,
    /// so deleted blocks stay out; a failed trim still snapshots. The snapshot carries the VM with its disks.
    fn snapshot(&self, id: &SandboxId) -> Result<SnapshotId, SandboxError>;
    /// Kills whatever still runs, and the VM's volumes go with it; destroying twice is not an error.
    fn destroy(&self, id: &SandboxId) -> Result<(), SandboxError>;
    fn drop_snapshot(&self, snapshot: &SnapshotId) -> Result<(), SandboxError>;
    /// Every sandbox and snapshot in the stand's namespace, also those of an earlier process.
    fn leftovers(&self) -> Result<Leftovers, SandboxError>;
    fn prune_images(&self, keep: &[ImageRef]) -> Result<(), SandboxError>;
}

#[derive(Debug, Clone)]
pub struct Launch {
    pub service: String,
    /// From 1.
    pub replica: u32,
    pub image: ImageRef,
    /// Whole vCPUs.
    pub cpus: u32,
    /// Bytes.
    pub memory: u64,
    pub env: BTreeMap<String, String>,
    pub workdir: Option<String>,
    /// `None`: the image's own ENTRYPOINT and CMD, as Compose combines them.
    pub entrypoint: Option<Vec<String>>,
    pub command: Option<Vec<String>>,
    pub mounts: Vec<Mount>,
    /// From `expose`: what the peers reach by name.
    pub ports: Vec<Exposed>,
    /// Every service of the version, with and without `expose`.
    pub peers: Vec<Peer>,
    pub outbound: Outbound,
    pub stop_signal: String,
    pub stop_grace: Duration,
    /// `run::span(&Stand)`.
    pub max_duration: Duration,
    pub seed: Option<Seed>,
}

/// For the setup cache key (plan, row 88).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pulled {
    /// The digest each image resolved to, in the order of `images`.
    pub digests: Vec<(ImageRef, String)>,
    /// The pinned `stand-guest` version; with `STOP_SLOP_STAND_GUEST`, `dev-` and the sha256 of that file.
    pub guest: String,
}

#[derive(Debug, Clone)]
pub enum Seed {
    /// The image, root disk and owned disks come from the snapshot; cpus, memory, outbound, ports and
    /// binds from the `Launch` (plan, row 87). `image` equals the snapshot's (`spec` and `run` keep it so);
    /// a `Mount::Volume` names a disk the snapshot carries, since a restore adds none. A bind path of the
    /// snapshotted VM missing from `mounts` is `Backend`, in `Fake` too.
    Snapshot(SnapshotId),
    /// A fresh owned ext4 version disk at the parent folder of `at`, and `tar` unpacked into `at` by the
    /// image's user before the command. Input goes through `stdin_pipe` in chunks up to 1 MiB: `stdin_bytes`
    /// above 4 MiB is one frame over the protocol cap and hangs with no reply and no error.
    Tree { tar: Utf8PathBuf, at: String },
}

#[derive(Debug, Clone)]
pub enum Mount {
    Bind {
        host: Utf8PathBuf,
        guest: String,
        read_only: bool,
    },
    Volume {
        name: VolumeName,
        guest: String,
    },
    /// An empty folder on the guest's root disk, owned by the image's user; `copy_out` brings it out.
    Scratch {
        guest: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exposed(pub u16);

/// `ports` is empty for a service without `expose`: its name resolves but does not answer.
#[derive(Debug, Clone)]
pub struct Peer {
    pub service: String,
    pub replicas: u32,
    pub ports: Vec<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outbound {
    Off,
    Public,
}

/// Opaque ids. `ImageRef` and `VolumeName` come from the caller; the others only from the port,
/// so their constructors are the adapters'.
macro_rules! name {
    ($name:ident, $new:vis) => {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name(String);

        impl $name {
            // A build without an adapter has no caller of the adapters' constructors.
            #[cfg_attr(not(any(feature = "fake", feature = "microsandbox")), allow(dead_code))]
            $new fn new(id: impl Into<String>) -> $name {
                $name(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

name!(SandboxId, pub(crate));
name!(ExecId, pub(crate));
name!(SnapshotId, pub(crate));
name!(VolumeName, pub);
name!(ImageRef, pub);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Waitable {
    Sandbox(SandboxId),
    Exec(ExecId),
}

/// `code` is `None` when the process was killed without one.
#[derive(Debug, Clone)]
pub struct Exited {
    pub what: Waitable,
    pub code: Option<i32>,
    pub at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ExecOutcome {
    Exited(i32),
    TimedOut,
}

/// `after` from the signal to the exit, or to the kill.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Stopped {
    pub after: Duration,
    pub killed: bool,
}

/// `target` is `alias:port`.
#[derive(Debug, Clone)]
pub struct Fault {
    pub target: String,
    pub mode: FaultMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultMode {
    Down,
    Slow(Duration),
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Metrics {
    /// vCPU time since `create`.
    pub cpu: Duration,
    pub memory_peak: u64,
    pub memory_limit: u64,
    /// Outgoing connections of this guest by `alias:port`.
    pub ports: Vec<PortStats>,
    pub unexposed: Vec<Unexposed>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PortStats {
    pub target: String,
    pub connections: u64,
    /// Open at the read.
    pub open: u64,
    pub bytes_out: u64,
    pub bytes_in: u64,
    /// Responses by class 1xx..5xx; `None` unless the connections began with an HTTP method.
    pub http: Option<[u64; 5]>,
    pub drops: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Unexposed {
    pub name: String,
    pub attempts: u64,
}

#[derive(Debug, Default)]
pub struct Leftovers {
    pub sandboxes: Vec<SandboxId>,
    pub snapshots: Vec<SnapshotId>,
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("image `{0}` is not in the registry")]
    ImageNotFound(ImageRef),
    #[error("registry: {message}")]
    Registry { message: String },
    #[error("sandbox `{0}` is gone")]
    Gone(SandboxId),
    /// Download, sha256, a guest call past its deadline, or a file of `stand-guest` (`Counters`, `StopReport`, `ExecStatus`).
    #[error("guest: {message}")]
    Guest { message: String },
    #[error("microsandbox: {message}")]
    Backend { message: String },
}
