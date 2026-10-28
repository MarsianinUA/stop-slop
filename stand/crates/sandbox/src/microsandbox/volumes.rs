//! A fresh VM with its binds, and the host folders behind them.

use std::time::Duration;

use ::microsandbox as msb;
use camino::Utf8Path;
use msb::sandbox::{MountBuilder, SandboxBuilder};

use super::guest::Binary;
use super::{CREATE, NAMESPACE, backend, destroy_vm, network, process};
use crate::{Launch, Mount, SandboxError};

pub(super) const PULL_MEMORY_MIB: u32 = 512;
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
            if let Mount::Bind {
                host,
                guest,
                read_only,
            } = mount
            {
                builder = builder.volume(guest, |m| bind(m, host, *read_only));
            }
        }
        builder
    })
    .await
    .map_err(|error| super::image_error(&launch.image, error))
}

fn bind(mount: MountBuilder, host: &Utf8Path, read_only: bool) -> MountBuilder {
    let mount = mount.bind(host.as_std_path());
    if read_only { mount.readonly() } else { mount }
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
