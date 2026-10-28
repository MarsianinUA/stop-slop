//! A fresh VM with its binds and owned disks, a VM from a snapshot, the sizes of the volumes and
//! the snapshot itself.
//!
//! A `Mount::Volume` is an ext4 disk the VM owns: it lives and dies with the VM and goes into its
//! snapshot. A restore takes the image, the root disk and the owned disks from the snapshot, and
//! the limits, the network, the ports and the host folders from its own call (plan rows 77, 87).

use std::time::Duration;

use ::microsandbox as msb;
use camino::Utf8Path;
use msb::sandbox::{MountBuilder, SandboxBuilder};

use super::guest::Binary;
use super::{CREATE, GUEST_CALL, Live, NAMESPACE, backend, destroy_vm, network, process, within};
use crate::{Launch, Mount, SandboxError, SnapshotId, VolumeName};

pub(super) const PULL_MEMORY_MIB: u32 = 512;
/// Sparse: the host pays for what the guest writes.
const VOLUME_DISK_MIB: u32 = 4 << 10;
const SNAPSHOT: Duration = Duration::from_secs(300);
/// The one `create` failure worth a retry: right after a cold pull, the VM process may exit before
/// its agent relay is up (spike state, 1 of about 40 creates).
const RELAY_RACE: &str = "before agent relay became available";

pub(super) async fn create_with_retry(
    name: &str,
    make: impl Fn() -> SandboxBuilder,
) -> Result<msb::Sandbox, msb::MicrosandboxError> {
    match create_once(name, make()).await {
        Err(error) if error.to_string().contains(RELAY_RACE) => create_once(name, make()).await,
        other => other,
    }
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
pub(super) async fn fresh(
    name: &str,
    launch: &Launch,
    published: &[(u16, u16)],
    folder: &Utf8Path,
    binary: &Binary,
) -> Result<msb::Sandbox, SandboxError> {
    let (cpus, memory, lifetime) = (cpus(launch)?, mib(launch.memory), secs(launch.max_duration));
    let (run, net) = (process::run_dir(folder), network::net_dir(folder));
    create_with_retry(name, || {
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
        builder
    })
    .await
    .map_err(|error| super::image_error(&launch.image, error))
}

/// Never `allow_missing_resources()`: a bind path of the snapshot with no folder in the `Launch`
/// is refused, not left an empty folder (plan row 87). A `Mount::Volume` names a disk the
/// snapshot carries; SDK 0.7.8 checks `captured()` only for full snapshots, so the adapter checks
/// the snapshot's owned disks itself.
pub(super) async fn restore(
    name: &str,
    launch: &Launch,
    snapshot: &SnapshotId,
    published: &[(u16, u16)],
    folder: &Utf8Path,
    binary: &Binary,
) -> Result<msb::Sandbox, SandboxError> {
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
    let mut builder = msb::Sandbox::restore_ref(found.reference())
        .name(name)
        .cpus(cpus(launch)?)
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
    let restored = within(CREATE, "restore a snapshot", builder.restore()).await;
    let restored = restored.and_then(|result| result.map_err(restore_error));
    if restored.is_err() {
        let _ = destroy_vm(name).await;
    }
    restored
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
