//! `stand-guest net`: the forwarder's entry. Under `Redirect` (in a guest) one listener takes every
//! TCP connection a netfilter REDIRECT rule sends it, and `SO_ORIGINAL_DST` names the target the
//! client meant. Under `Direct` (host tests, no root) a listener binds each route's `address:port`.
//! The relay is the same; only how a connection arrives and how its target is found differ.

use std::net::{SocketAddr, TcpListener};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use guest::{Entry, Route, Routes};

use crate::relay::Relay;

static TERM: AtomicBool = AtomicBool::new(false);

pub fn run(routes: &Routes) -> ExitCode {
    let relay = Relay::new(&routes.upstream_host, &routes.routes, &routes.unexposed);
    let started = match routes.entry {
        Entry::Redirect => start_redirect(&relay, routes),
        Entry::Direct => start_direct(&relay, &routes.routes),
    };
    if let Err(message) = started {
        return crate::failed("stand-guest net", message);
    }
    relay.watch_fault(routes.fault.clone());
    // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
    unsafe {
        libc::signal(
            libc::SIGTERM,
            on_term as extern "C" fn(libc::c_int) as libc::sighandler_t,
        )
    };
    relay.write_counters(&routes.counters, &TERM);
    ExitCode::SUCCESS
}

extern "C" fn on_term(_: libc::c_int) {
    TERM.store(true, Ordering::Release);
}

/// The rules are in before `run` writes the first counters file, so once that file is there names
/// reach the forwarder.
#[cfg(target_os = "linux")]
fn start_redirect(relay: &Arc<Relay>, routes: &Routes) -> Result<(), String> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| format!("bind the forwarder: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("the forwarder's port: {error}"))?
        .port();
    let mut addresses: Vec<_> = routes
        .routes
        .iter()
        .map(|route| route.address)
        .chain(routes.unexposed.iter().map(|name| name.address))
        .collect();
    addresses.sort();
    addresses.dedup();
    crate::nf::install(port, &addresses)
        .map_err(|error| format!("install the netfilter rules: {error}"))?;
    let relay = Arc::clone(relay);
    thread::spawn(move || {
        for client in listener.incoming().flatten() {
            relay.serve_redirected(client);
        }
    });
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn start_redirect(_: &Arc<Relay>, _: &Routes) -> Result<(), String> {
    Err("--redirect needs netfilter, which only Linux has; on this host use --direct".into())
}

/// The listener of `routes[i]` serves target `i`.
fn start_direct(relay: &Arc<Relay>, routes: &[Route]) -> Result<(), String> {
    for (index, route) in routes.iter().enumerate() {
        let address = SocketAddr::from((route.address, route.port));
        let listener =
            TcpListener::bind(address).map_err(|error| format!("bind {address}: {error}"))?;
        let relay = Arc::clone(relay);
        thread::spawn(move || {
            for client in listener.incoming().flatten() {
                relay.serve(index, client);
            }
        });
    }
    Ok(())
}
