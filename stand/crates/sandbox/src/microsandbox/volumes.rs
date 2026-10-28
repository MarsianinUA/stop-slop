//! A fresh VM with its binds and owned disks, the version tree, a VM from a snapshot, the sizes of
//! the volumes and the snapshot itself.
//!
//! A `Mount::Volume` and the version disk are ext4 disks the VM owns: they live and die with the VM
//! and go into its snapshot. A restore takes the image, the root disk and the owned disks from the
//! snapshot, and the limits, the network, the ports and the host folders from its own call (plan
//! rows 77, 86, 87).

use std::path::PathBuf;
use std::time::Duration;

use ::guest::{Command, TrimArgs};
use ::microsandbox as msb;
use camino::Utf8Path;
use msb::sandbox::{MountBuilder, SandboxBuilder};
use tokio::io::AsyncReadExt;

use super::guest::Binary;
use super::process::Owner;
use super::{
    CREATE, GUEST_CALL, Live, NAMESPACE, backend, destroy_vm, guest, network, process, within,
};
use crate::{Launch, Mount, SandboxError, Seed, SnapshotId, VolumeName};

pub(super) const PULL_MEMORY_MIB: u32 = 512;
/// Sparse: the host pays for what the guest writes.
const VOLUME_DISK_MIB: u32 = 4 << 10;
const TREE_DISK_MIB: u32 = 8 << 10;
/// `stdin_bytes` sends its input as one frame, and a frame over 4 MiB hangs with no reply and no
/// error (spike deps).
const CHUNK: usize = 1 << 20;
const TREE: Duration = Duration::from_secs(300);
/// The first FITRIM after a mount walks all free space of the disk.
const TRIM: Duration = Duration::from_secs(120);
const SNAPSHOT: Duration = Duration::from_secs(300);
/// Right after a cold pull, the VM process may exit before its agent relay is up (spike state, 1
/// of about 40 creates).
const RELAY_RACE: &str = "before agent relay became available";

/// The `create` failures worth one retry: the relay race, and on Windows two VMs of a batch that
/// bind one writable host folder. Each checks the folder by writing, reading back and removing
/// the same `msb._probe` stream on it (`microsandbox-filesystem` `stat_store.rs`), so the second
/// can find the stream gone and fail `mount <tag>: ... (os error 2)`; a moment later it passes.
pub(super) async fn create_with_retry(
    name: &str,
    make: impl Fn() -> SandboxBuilder,
) -> Result<msb::Sandbox, msb::MicrosandboxError> {
    match create_once(name, make()).await {
        Err(error) if lost_a_race(&error.to_string()) => create_once(name, make()).await,
        other => other,
    }
}

fn lost_a_race(error: &str) -> bool {
    let shared_bind = error.contains(": mount ") && error.ends_with("(os error 2)");
    error.contains(RELAY_RACE) || (cfg!(windows) && shared_bind)
}

/// A failed `create` may leave its VM behind, so it goes; the error that made it fail is the one
/// returned.
async fn create_once(
    name: &str,
    builder: SandboxBuilder,
) -> Result<msb::Sandbox, msb::MicrosandboxError> {
    let created = tokio::time::timeout(CREATE, builder.create())
        .await
        .unwrap_or_else(|_| {
            Err(msb::MicrosandboxError::Custom(format!(
                "create: no answer within {} s",
                CREATE.as_secs()
            )))
        });
    if created.is_err() {
        let _ = destroy_vm(name).await;
    }
    created
}

/// `folder` is the adapter's folder of the VM, not a bind itself; `published` is (host, guest).
/// Gives the VM and the guest paths of its owned disks.
pub(super) async fn fresh(
    name: &str,
    launch: &Launch,
    published: &[(u16, u16)],
    folder: &Utf8Path,
    binary: &Binary,
) -> Result<(msb::Sandbox, Vec<String>), SandboxError> {
    let (cpus, memory, lifetime) = (cpus(launch)?, mib(launch.memory), secs(launch.max_duration));
    let (run, net) = (process::run_dir(folder), network::net_dir(folder));
    let tree = match &launch.seed {
        Some(Seed::Tree { at, .. }) => Some(tree_disk(at)),
        _ => None,
    };
    let made = create_with_retry(name, || {
        let mut builder = msb::Sandbox::builder(name)
            .image(launch.image.as_str())
            .cpus(cpus)
            .memory(memory)
            .max_duration(lifetime)
            .label(NAMESPACE, "1")
            .network(|network| network.policy(network::policy(launch.outbound)))
            .volume(super::guest::DIR, |m| {
                m.bind(binary.dir.as_std_path()).readonly()
            })
            .volume(process::RUN_DIR, |m| m.bind(run.as_std_path()))
            .volume(network::NET_DIR, |m| m.bind(net.as_std_path()));
        for &(host, guest) in published {
            builder = builder.port(host, guest);
        }
        for mount in &launch.mounts {
            builder = match mount {
                Mount::Bind {
                    host,
                    guest,
                    read_only,
                } => builder.volume(guest, |m| bind(m, host, *read_only)),
                Mount::Volume { guest, .. } => builder.volume(guest, |m| {
                    m.owned_with(|disk| disk.disk().size(VOLUME_DISK_MIB))
                }),
                Mount::Scratch { .. } => builder,
            };
        }
        if let Some(disk) = tree {
            builder = builder.volume(disk, |m| {
                m.owned_with(|disk| disk.disk().size(TREE_DISK_MIB))
            });
        }
        builder
    })
    .await
    .map_err(|error| super::image_error(&launch.image, error))?;
    let volumes = launch.mounts.iter().filter_map(|mount| match mount {
        Mount::Volume { guest, .. } => Some(guest.clone()),
        _ => None,
    });
    let owned = volumes.chain(tree.map(str::to_owned)).collect();
    Ok((made, owned))
}

/// The version disk is mounted at the parent of the tree: `/w` for `/w/app`.
fn tree_disk(at: &str) -> &str {
    Utf8Path::new(at).parent().map_or("/", Utf8Path::as_str)
}

/// Root gives the disk and `at` to the image's user (the package store goes on the disk beside the
/// tree), then the image's own `tar` unpacks as that user, so the files are the user's.
pub(super) async fn seed_tree(
    vm: &msb::Sandbox,
    tar: &Utf8Path,
    at: &str,
    owner: Owner,
) -> Result<(), SandboxError> {
    let fs = vm.fs();
    within(GUEST_CALL, "make the tree folder", fs.mkdir(at))
        .await?
        .map_err(backend)?;
    for folder in [tree_disk(at), at] {
        process::give(vm, folder, owner).await?;
    }
    let mut file = tokio::fs::File::open(tar)
        .await
        .map_err(|error| backend(format!("{tar}: {error}")))?;
    let args = ["-x", "-f", "-", "-C", at];
    let unpack = async {
        let mut handle = vm
            .exec_stream_with("tar", |options| {
                options.args(args).stdin_pipe().timeout(TREE)
            })
            .await
            .map_err(backend)?;
        let sink = handle
            .take_stdin()
            .ok_or_else(|| backend("tar: the SDK gave no stdin to write to"))?;
        let mut chunk = vec![0; CHUNK];
        // A `tar` that stopped early refuses the rest; its exit and stderr say why.
        let mut sent = Ok(());
        loop {
            let read = file
                .read(&mut chunk)
                .await
                .map_err(|error| backend(format!("{tar}: {error}")))?;
            // An empty write is EOF to the agent, so the loop never sends one.
            if read == 0 {
                break;
            }
            if let Err(error) = sink.write(&chunk[..read]).await {
                sent = Err(error);
                break;
            }
        }
        if sent.is_ok() {
            sent = sink.close().await;
        }
        let output = handle.collect().await.map_err(backend)?;
        Ok::<_, SandboxError>((sent, output))
    };
    let (sent, output) = within(TREE + GUEST_CALL, "unpack the version tree", unpack).await??;
    if output.status().code != 0 {
        return Err(SandboxError::Guest {
            message: format!(
                "tar -x into {at} exited with {}: {}",
                output.status().code,
                String::from_utf8_lossy(output.stderr_bytes()).trim()
            ),
        });
    }
    sent.map_err(backend)
}

/// Never `allow_missing_resources()`: a bind path of the snapshot with no folder in the `Launch`
/// is refused, not left an empty folder (plan row 87). A `Mount::Volume` names a disk the
/// snapshot carries; SDK 0.7.8 checks `captured()` only for full snapshots, so the adapter checks
/// the snapshot's owned disks itself. Every disk the snapshot carries comes back, the version disk
/// among them, named or not, so the guest paths given are the snapshot's.
pub(super) async fn restore(
    name: &str,
    launch: &Launch,
    snapshot: &SnapshotId,
    published: &[(u16, u16)],
    folder: &Utf8Path,
    binary: &Binary,
) -> Result<(msb::Sandbox, Vec<String>), SandboxError> {
    let found = listed(snapshot).await?;
    let found = found.ok_or_else(|| backend(format!("no snapshot `{snapshot}`")))?;
    let opened = within(GUEST_CALL, "open a snapshot", found.open())
        .await?
        .map_err(backend)?;
    let carried = opened.manifest().owned_volumes().map_err(backend)?;
    for mount in &launch.mounts {
        if let Mount::Volume {
            name: volume,
            guest,
        } = mount
            && !carried.iter().any(|disk| disk.mount.guest == *guest)
        {
            return Err(backend(format!(
                "snapshot `{snapshot}` carries no volume at `{guest}` for `{volume}`, and a \
                 restore adds none"
            )));
        }
    }
    let (run, net) = (process::run_dir(folder), network::net_dir(folder));
    let cpus = cpus(launch)?;
    let make = || {
        let mut builder = msb::Sandbox::restore_ref(found.reference())
            .name(name)
            .cpus(cpus)
            .memory(mib(launch.memory))
            .max_duration(secs(launch.max_duration))
            .network_policy(network::policy(launch.outbound))
            .volume(super::guest::DIR, |m| {
                m.bind(binary.dir.as_std_path()).readonly()
            })
            .volume(process::RUN_DIR, |m| m.bind(run.as_std_path()))
            .volume(network::NET_DIR, |m| m.bind(net.as_std_path()));
        for &(host, guest) in published {
            builder = builder.port(host, guest);
        }
        for mount in &launch.mounts {
            builder = match mount {
                Mount::Bind {
                    host,
                    guest,
                    read_only,
                } => builder.volume(guest, |m| bind(m, host, *read_only)),
                Mount::Volume { guest, .. } => builder.volume(guest, MountBuilder::captured),
                Mount::Scratch { .. } => builder,
            };
        }
        builder
    };
    let attempt = || async {
        let restored = within(CREATE, "restore a snapshot", make().restore()).await;
        let restored = restored.and_then(|result| result.map_err(restore_error));
        if restored.is_err() {
            let _ = destroy_vm(name).await;
        }
        restored
    };
    let restored = match attempt().await {
        Err(error) if lost_a_race(&error.to_string()) => attempt().await,
        other => other,
    };
    let owned = carried.into_iter().map(|disk| disk.mount.guest).collect();
    Ok((restored?, owned))
}

/// A VM from a snapshot is a detached process (`CREATE_BREAKAWAY_FROM_JOB`), and Windows refuses
/// it under a parent job without breakaway, which is how `cargo run` and `cargo test` keep their
/// children.
fn restore_error(error: msb::MicrosandboxError) -> SandboxError {
    match &error {
        msb::MicrosandboxError::Io(io)
            if cfg!(windows) && io.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            backend(format!(
                "{error} (a VM from a snapshot cannot start under a Windows job without \
                 breakaway, such as `cargo run` or `cargo test`)"
            ))
        }
        _ => backend(error),
    }
}

/// By its digest in the listing. The SDK's own lookup by digest refuses on Windows ("has 2 local
/// copies"): a capture is indexed under the path it had before it existed and again under its
/// canonical `\\?\` path, and only the second is listed.
pub(super) async fn listed(
    snapshot: &SnapshotId,
) -> Result<Option<msb::SnapshotHandle>, SandboxError> {
    let all = within(GUEST_CALL, "list the snapshots", msb::Snapshot::list())
        .await?
        .map_err(backend)?;
    Ok(all
        .into_iter()
        .find(|handle| handle.digest() == snapshot.as_str()))
}

fn bind(mount: MountBuilder, host: &Utf8Path, read_only: bool) -> MountBuilder {
    let mount = mount.bind(host.as_std_path());
    if read_only { mount.readonly() } else { mount }
}

/// KiB of used blocks times 1024: busybox `du` (the alpine images) has no `-b`.
pub(super) async fn sizes(live: &Live) -> Result<Vec<(VolumeName, u64)>, SandboxError> {
    let mut sizes = Vec::new();
    for mount in &live.launch.mounts {
        let Mount::Volume { name, guest } = mount else {
            continue;
        };
        let args = vec!["-sk".to_owned(), guest.clone()];
        let output = process::root(&live.vm, "du", args, GUEST_CALL).await?;
        let text = String::from_utf8_lossy(output.stdout_bytes());
        let kib = text
            .split_whitespace()
            .next()
            .and_then(|n| n.parse::<u64>().ok());
        match kib {
            Some(kib) if output.status().code == 0 => sizes.push((name.clone(), kib * 1024)),
            _ => {
                return Err(SandboxError::Guest {
                    message: format!(
                        "du -sk {guest} exited with {}: `{}` {}",
                        output.status().code,
                        text.trim(),
                        String::from_utf8_lossy(output.stderr_bytes()).trim()
                    ),
                });
            }
        }
    }
    Ok(sizes)
}

/// Of the running VM, so `logs`, `metrics` and `copy_out` still work after it. In a group of its
/// own: the SDK groups the snapshots of one VM by default and refuses to drop the group's head
/// while another member remains.
pub(super) async fn snapshot(live: &Live, name: &str) -> Result<SnapshotId, SandboxError> {
    trim(live).await;
    let capture = msb::Snapshot::builder(name)
        .from_sandbox(&live.name)
        .group(name)
        .label(NAMESPACE, "1")
        .create();
    let taken = within(SNAPSHOT, "take a snapshot", capture)
        .await?
        .map_err(backend)?;
    Ok(SnapshotId::new(taken.digest().to_owned()))
}

/// FITRIM on every owned disk, so the blocks of deleted files stay out of the snapshot. A failure
/// only leaves them in, so it goes to the log, never stderr (`cli` owns it), and the snapshot is
/// taken anyway. The root disk takes no discard and is never named.
async fn trim(live: &Live) {
    if live.owned.is_empty() {
        return;
    }
    let mounts = live.owned.iter().map(PathBuf::from).collect();
    let args = Command::Trim(TrimArgs { mounts }).to_args();
    let failure = match process::root(&live.vm, guest::BIN, args, TRIM).await {
        Ok(output) if output.status().code == 0 => return,
        Ok(output) => format!(
            "exit {}: {}",
            output.status().code,
            String::from_utf8_lossy(output.stderr_bytes()).trim()
        ),
        Err(error) => error.to_string(),
    };
    tracing::warn!(
        vm = %live.name,
        "stand-guest trim failed, the snapshot keeps the deleted blocks: {failure}"
    );
}

/// Writable by any guest user: on Linux and macOS hosts a bind shows the host folder's owner and
/// mode, and the image's user may be anyone.
pub(super) fn make_folder(folder: &Utf8Path) -> Result<(), SandboxError> {
    std::fs::create_dir_all(folder).map_err(|error| backend(format!("{folder}: {error}")))?;
    open_to_every_user(folder)
}

#[cfg(unix)]
pub(super) fn open_to_every_user(folder: &Utf8Path) -> Result<(), SandboxError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o777))
        .map_err(|error| backend(format!("{folder}: {error}")))
}

/// A Windows bind shows every file as `rwxrwxrwx` in the guest.
#[cfg(not(unix))]
pub(super) fn open_to_every_user(_: &Utf8Path) -> Result<(), SandboxError> {
    Ok(())
}

fn cpus(launch: &Launch) -> Result<u8, SandboxError> {
    u8::try_from(launch.cpus).map_err(|_| {
        backend(format!(
            "{} vCPUs for `{}`: microsandbox takes up to 255",
            launch.cpus, launch.service
        ))
    })
}

fn mib(bytes: u64) -> u32 {
    u32::try_from(bytes.div_ceil(1 << 20)).unwrap_or(u32::MAX)
}

fn secs(duration: Duration) -> u64 {
    duration.as_secs() + u64::from(duration.subsec_nanos() > 0)
}
