//! Names, published ports, the network policy, `stand-guest net` and its fault switch.
//!
//! Each peer name gets a loopback address in the guest's hosts file; `net`'s REDIRECT rule sends a
//! connection to that address into the forwarder, which dials `host.microsandbox.internal` on the
//! host port the peer's replica publishes (architecture plan, rows 27, 80).

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ::guest::{Command, Entry, FaultSwitch, FaultSwitchMode, Route, Routes, UnexposedName};
use ::microsandbox as msb;
use camino::{Utf8Path, Utf8PathBuf};
use msb::sandbox::FsSetAttrs;

use super::{GUEST_CALL, Live, backend, guest, process, within};
use crate::{Fault, FaultMode, Launch, Outbound, SandboxError};

/// Root writes it: `net` its counters, the adapter its fault switch.
pub(super) const NET_DIR: &str = "/stand/net";
const COUNTERS: &str = "counters.json";
const FAULT: &str = "fault.json";
const UPSTREAM: &str = "host.microsandbox.internal";
const HOSTS: &str = "/etc/hosts";
const HOSTS_BEGIN: &str = "# stop-slop-stand begin";
const HOSTS_END: &str = "# stop-slop-stand end";
/// 127.0.0.2 to 127.0.0.254.
const MAX_NAMES: usize = 253;
/// `net` reads the switch every 100 ms and says nothing back.
const FAULT_SETTLE: Duration = Duration::from_millis(200);
const FREE_PORT_TRIES: usize = 16;

/// The host side of `NET_DIR` in the adapter's folder of a VM.
pub(super) fn net_dir(folder: &Utf8Path) -> Utf8PathBuf {
    folder.join("net")
}

pub(super) fn counters(folder: &Utf8Path) -> Utf8PathBuf {
    net_dir(folder).join(COUNTERS)
}

/// One host port per (service, replica, port) for the adapter's life, so a VM created before its
/// peer already routes to it, and a later VM of the same replica publishes the same port. Two live
/// VMs of one (service, replica) would clash on it, so `run` never holds them at once.
#[derive(Default)]
pub(super) struct Ports(BTreeMap<(String, u32, u16), u16>);

impl Ports {
    fn of(&mut self, service: &str, replica: u32, port: u16) -> Result<u16, SandboxError> {
        let key = (service.to_owned(), replica, port);
        if let Some(host) = self.0.get(&key) {
            return Ok(*host);
        }
        // The listener closes at once, so the OS may give the same port again to a later key.
        for _ in 0..FREE_PORT_TRIES {
            let host = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                .and_then(|listener| listener.local_addr())
                .map_err(|error| backend(format!("find a free host port: {error}")))?
                .port();
            if !self.0.values().any(|&taken| taken == host) {
                self.0.insert(key, host);
                return Ok(host);
            }
        }
        Err(backend(format!(
            "no free host port the adapter has not given out, in {FREE_PORT_TRIES} tries"
        )))
    }
}

pub(super) struct Wiring {
    /// (host, guest) per `expose` of this VM.
    pub published: Vec<(u16, u16)>,
    names: Vec<(String, Ipv4Addr)>,
    routes: Routes,
}

pub(super) fn wire(ports: &mut Ports, launch: &Launch) -> Result<Wiring, SandboxError> {
    let published = launch
        .ports
        .iter()
        .map(|exposed| {
            Ok((
                ports.of(&launch.service, launch.replica, exposed.0)?,
                exposed.0,
            ))
        })
        .collect::<Result<Vec<_>, SandboxError>>()?;
    let mut peers: Vec<_> = launch.peers.iter().collect();
    peers.sort_by(|a, b| a.service.cmp(&b.service));
    if peers.len() > MAX_NAMES {
        return Err(backend(format!(
            "{} services: at most {MAX_NAMES} names fit 127.0.0.2-254",
            peers.len()
        )));
    }
    let mut names = Vec::new();
    let mut routes = Vec::new();
    let mut unexposed = Vec::new();
    for (address, peer) in (2..=u8::MAX)
        .map(|last| Ipv4Addr::new(127, 0, 0, last))
        .zip(peers)
    {
        names.push((peer.service.clone(), address));
        if peer.ports.is_empty() {
            unexposed.push(UnexposedName {
                name: peer.service.clone(),
                address,
            });
        }
        for &port in &peer.ports {
            let upstream = (1..=peer.replicas)
                .map(|replica| ports.of(&peer.service, replica, port))
                .collect::<Result<Vec<_>, _>>()?;
            routes.push(Route {
                name: peer.service.clone(),
                port,
                address,
                upstream,
            });
        }
    }
    let routes = Routes {
        upstream_host: UPSTREAM.to_owned(),
        entry: Entry::Redirect,
        routes,
        unexposed,
        counters: PathBuf::from(format!("{NET_DIR}/{COUNTERS}")),
        fault: PathBuf::from(format!("{NET_DIR}/{FAULT}")),
    };
    Ok(Wiring {
        published,
        names,
        routes,
    })
}

/// `Off` still reaches the host: the forwarder's upstream is there. Never `default_deny()`.
pub(super) fn policy(outbound: Outbound) -> msb::NetworkPolicy {
    match outbound {
        Outbound::Off => msb::NetworkPolicy::from_profiles([msb::NetworkProfile::Host]),
        Outbound::Public => msb::NetworkPolicy::from_profiles([
            msb::NetworkProfile::Public,
            msb::NetworkProfile::Host,
        ]),
    }
}

/// Before the service: the names, `/stand/net` closed to the service's user, then `net` with its
/// REDIRECT rules in place, since a service that binds a wildcard listener first would take the
/// connections to its peer's port (spike net).
pub(super) async fn start(
    vm: &msb::Sandbox,
    wiring: &Wiring,
    folder: &Utf8Path,
) -> Result<(), SandboxError> {
    write_hosts(vm, &wiring.names).await?;
    keep_to_root(vm).await?;
    let args = Command::Net(wiring.routes.clone()).to_args();
    let start = vm.exec_detached_with(guest::BIN, |options| {
        options.args(args).user("root").stdin_null()
    });
    let job = within(GUEST_CALL, "start stand-guest net", start)
        .await?
        .map_err(backend)?;
    match process::wait_for_file(&job, &counters(folder)).await? {
        None => Ok(()),
        Some(info) => Err(process::ended(&job, "stand-guest net", &info).await),
    }
}

/// The switch goes in as a temporary file and is renamed there, so the guest's own view of the
/// bind has it at once, whatever virtio-fs caches of the host's, and `net` never reads a part.
/// Every VM gets its copy before any rename, so a failed copy turns the fault on nowhere, and
/// the wait is paid once.
pub(super) async fn fault(lives: &[Arc<Live>], fault: &Fault) -> Result<(), SandboxError> {
    let mode = match fault.mode {
        FaultMode::Down => FaultSwitchMode::Down,
        FaultMode::Slow(delay) => FaultSwitchMode::Slow {
            delay_ms: u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
        },
    };
    let switch = FaultSwitch {
        target: fault.target.clone(),
        mode,
    };
    let (tmp, path) = (
        format!("{NET_DIR}/{FAULT}.tmp"),
        format!("{NET_DIR}/{FAULT}"),
    );
    for live in lives {
        // Outside the binds, so only the renamed copy shows in the guest.
        let staged = live.folder.join(FAULT);
        switch
            .write(staged.as_std_path())
            .map_err(|error| backend(format!("{staged}: {error}")))?;
        let fs = live.vm.fs();
        let copy = fs.copy_from_host(&staged, &tmp);
        let copied = within(GUEST_CALL, "copy the fault switch in", copy).await;
        let _ = std::fs::remove_file(&staged);
        copied?.map_err(backend)?;
    }
    for live in lives {
        let fs = live.vm.fs();
        within(GUEST_CALL, "turn the fault on", fs.rename(&tmp, &path))
            .await?
            .map_err(backend)?;
    }
    tokio::time::sleep(FAULT_SETTLE).await;
    Ok(())
}

/// A VM from a snapshot carries the hosts file of the VM it was taken from, so the block is
/// replaced, not appended. Through the agent, which is root, so no shell or `sed` in the image.
async fn write_hosts(vm: &msb::Sandbox, names: &[(String, Ipv4Addr)]) -> Result<(), SandboxError> {
    let fs = vm.fs();
    let current = within(GUEST_CALL, "read /etc/hosts", fs.read_to_string(HOSTS))
        .await?
        .map_err(backend)?;
    let next = hosts_with(&current, names);
    within(
        GUEST_CALL,
        "write /etc/hosts",
        fs.write(HOSTS, next.as_bytes()),
    )
    .await?
    .map_err(backend)
}

fn hosts_with(current: &str, names: &[(String, Ipv4Addr)]) -> String {
    let mut kept = String::new();
    let mut inside = false;
    for line in current.lines() {
        match line {
            HOSTS_BEGIN => inside = true,
            HOSTS_END => inside = false,
            _ if !inside => {
                kept.push_str(line);
                kept.push('\n');
            }
            _ => {}
        }
    }
    kept.push_str(HOSTS_BEGIN);
    kept.push('\n');
    for (name, address) in names {
        kept.push_str(&format!("{address} {name}\n"));
    }
    kept.push_str(HOSTS_END);
    kept.push('\n');
    kept
}

/// A bind shows its host folder as `rwxrwxrwx` on Windows, and as the image user's own on Linux
/// and macOS (stat virtualization gives the host owner's files to that user). Owner and mode set
/// from inside the guest stay in the SDK's overlay of the bind and hold for the guest's kernel.
async fn keep_to_root(vm: &msb::Sandbox) -> Result<(), SandboxError> {
    let root_only = FsSetAttrs {
        mode: Some(0o755),
        uid: Some(0),
        gid: Some(0),
        ..FsSetAttrs::default()
    };
    let fs = vm.fs();
    within(
        GUEST_CALL,
        "give /stand/net to root",
        fs.set_stat(NET_DIR, false, root_only),
    )
    .await?
    .map_err(backend)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hosts_block_is_replaced_and_the_rest_kept() {
        let db = [("db".to_owned(), Ipv4Addr::new(127, 0, 0, 2))];
        let first = hosts_with("127.0.0.1 localhost\n", &db);
        let api = [("api".to_owned(), Ipv4Addr::new(127, 0, 0, 2))];
        assert_eq!(
            hosts_with(&first, &api),
            "127.0.0.1 localhost\n# stop-slop-stand begin\n127.0.0.2 api\n# stop-slop-stand end\n"
        );
    }

    #[test]
    fn each_replica_port_keeps_a_host_port_of_its_own() {
        let mut ports = Ports::default();
        let given: Vec<u16> = (1..=64)
            .map(|replica| ports.of("srv", replica, 8080).unwrap())
            .collect();
        let mut distinct = given.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), given.len(), "{given:?}");
        assert_eq!(ports.of("srv", 7, 8080).unwrap(), given[6]);
    }
}
