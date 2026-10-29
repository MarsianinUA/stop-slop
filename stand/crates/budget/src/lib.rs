//! Whether the host can take a stand now: its CPUs, memory and free disk against what the stand
//! asks for.

use std::fmt;
use std::fs;

use camino::Utf8Path;
use sysinfo::{CpuRefreshKind, DiskRefreshKind, Disks, MemoryRefreshKind, System};

/// The share of the host when `stand.toml` sets none, and the most it may set.
const SHARE: u8 = 50;
const MAX_SHARE: u8 = 80;

const GIB: u64 = 1 << 30;
/// Memory the host keeps for itself beside the guests.
const MEMORY_RESERVE: u64 = 2 * GIB;
/// Free disk the stand keeps on the volume of its folder: snapshots of both versions and of the
/// setup volumes, which the setup cache keeps between runs, and a version disk per live VM.
const DISK_FLOOR: u64 = 20 * GIB;

#[derive(Debug, Clone, Copy)]
pub struct Host {
    /// Logical CPUs.
    pub cpus: u32,
    pub memory: u64,
    /// What a new VM can take, page cache included.
    pub free_memory: u64,
    /// On the volume of the stand's folder.
    pub free_disk: u64,
}

impl Host {
    pub fn read(tool_dir: &Utf8Path) -> Result<Host, HostError> {
        let mut system = System::new();
        system.refresh_cpu_list(CpuRefreshKind::nothing());
        system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
        let cpus = u32::try_from(system.cpus().len()).unwrap_or(u32::MAX);
        let memory = system.total_memory();
        if cpus == 0 || memory == 0 {
            return Err(HostError(format!(
                "{cpus} CPUs and {memory} bytes of memory"
            )));
        }
        Ok(Host {
            cpus,
            memory,
            free_memory: system.available_memory(),
            free_disk: free_disk(tool_dir)?,
        })
    }
}

/// The disk whose mount point is the longest prefix of the folder. Both sides are canonical: a
/// folder behind a link lies on the disk of its target, and Windows gives both the `\\?\` form.
fn free_disk(tool_dir: &Utf8Path) -> Result<u64, HostError> {
    let dir =
        fs::canonicalize(tool_dir).map_err(|error| HostError(format!("{tool_dir}: {error}")))?;
    let disks = Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_storage());
    disks
        .list()
        .iter()
        .filter_map(|disk| {
            Some((
                fs::canonicalize(disk.mount_point()).ok()?,
                disk.available_space(),
            ))
        })
        .filter(|(mount, _)| dir.starts_with(mount))
        .max_by_key(|(mount, _)| mount.components().count())
        .map(|(_, free)| free)
        .ok_or_else(|| HostError(format!("no disk holds {tool_dir}")))
}

#[derive(Debug, Clone, Copy)]
pub struct Share(u8);

impl Share {
    /// `None` is 50; 1..=80 passes.
    pub fn new(percent: Option<u8>) -> Result<Share, BudgetError> {
        match percent.unwrap_or(SHARE) {
            percent @ 1..=MAX_SHARE => Ok(Share(percent)),
            percent => Err(BudgetError::Share(percent)),
        }
    }

    pub fn percent(self) -> u8 {
        self.0
    }
}

/// What the stand asks for, from `spec::Stand::demand()`.
#[derive(Debug, Clone, Copy)]
pub struct Demand {
    pub cpus: u32,
    pub memory: u64,
}

/// CPUs up to the share of the logical ones; memory up to the lower of the share of all of it and
/// the free memory less 2 GiB; at least 20 GiB of free disk. The first that fails refuses.
pub fn check(demand: Demand, host: &Host, share: Share) -> Result<Admission, Refusal> {
    let refusal = |resource, requested, allowed, total, free| Refusal {
        resource,
        requested,
        allowed,
        total,
        free,
        share: share.0,
    };
    let all = u64::from(host.cpus);
    let cpus = part(all, share);
    if u64::from(demand.cpus) > cpus {
        return Err(refusal(Resource::Cpu, demand.cpus.into(), cpus, all, all));
    }
    let memory = part(host.memory, share).min(host.free_memory.saturating_sub(MEMORY_RESERVE));
    if demand.memory > memory {
        return Err(refusal(
            Resource::Memory,
            demand.memory,
            memory,
            host.memory,
            host.free_memory,
        ));
    }
    if disk_low(host) {
        let free = host.free_disk;
        return Err(refusal(Resource::Disk, DISK_FLOOR, free, free, free));
    }
    Ok(Admission {
        cpus: demand.cpus,
        memory: demand.memory,
    })
}

/// Free disk under the 20 GiB the stand keeps free; the floor lives only here.
pub fn disk_low(host: &Host) -> bool {
    host.free_disk < DISK_FLOOR
}

fn part(whole: u64, share: Share) -> u64 {
    (u128::from(whole) * u128::from(share.0) / 100) as u64
}

/// The demand that fit: the peak of a run, CPUs as whole vCPUs.
#[derive(Debug)]
pub struct Admission {
    pub cpus: u32,
    pub memory: u64,
}

/// `Host` counts no free CPUs and no size of the disk: `total` and `free` of a CPU refusal are
/// the logical CPUs, of a disk refusal the free disk, and its `requested` is the floor.
#[derive(Debug)]
pub struct Refusal {
    pub resource: Resource,
    pub requested: u64,
    pub allowed: u64,
    pub total: u64,
    pub free: u64,
    pub share: u8,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let share = self.share;
        match self.resource {
            Resource::Cpu => write!(
                f,
                "cpu: requested {} vCPUs, allowed {}: {share}% of {} logical CPUs",
                self.requested, self.allowed, self.total
            ),
            Resource::Memory => write!(
                f,
                "memory: requested {}, allowed {}: the lower of {share}% of {} and {} free less {} \
                 for the host",
                Bytes(self.requested),
                Bytes(self.allowed),
                Bytes(self.total),
                Bytes(self.free),
                Bytes(MEMORY_RESERVE)
            ),
            Resource::Disk => write!(
                f,
                "disk: {} free on the volume of the stand's folder, under the {} it keeps free \
                 for snapshots",
                Bytes(self.free),
                Bytes(self.requested)
            ),
        }
    }
}

impl std::error::Error for Refusal {}

/// MiB under 1 GiB, else GiB with a tenth, rounded down.
struct Bytes(u64);

impl fmt::Display for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 < GIB {
            write!(f, "{} MiB", self.0 >> 20)
        } else {
            let tenths = u128::from(self.0) * 10 / u128::from(GIB);
            write!(f, "{}.{} GiB", tenths / 10, tenths % 10)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    Cpu,
    Memory,
    Disk,
}

/// `cli` reports it as the `ConfigError` of `host_share`, with the file of `stand.toml`.
#[derive(Debug, thiserror::Error)]
pub enum BudgetError {
    #[error("{0} is outside 1-80")]
    Share(u8),
}

#[derive(Debug, thiserror::Error)]
#[error("cannot read the host: {0}")]
pub struct HostError(String);
