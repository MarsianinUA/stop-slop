//! The relay, shared by both entries. A connection to an exposed name goes round-robin to one of
//! the upstream host ports, and its bytes are counted as they pass. A `FaultSwitch` file turns
//! one target `down` or `slow` while the relay runs.

use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4, TcpStream, ToSocketAddrs};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use guest::{
    Counters, FaultSwitch, FaultSwitchMode, Route, TargetCounters, UnexposedCount, UnexposedName,
};

use crate::sniff::HttpSniff;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The network limit of microsandbox closes a connection 0.5-1 s after it opens; an upstream that
/// closes a quiet connection later, on an idle timeout, has not hit it.
const DROP_WITHIN: Duration = Duration::from_secs(3);
const COUNTERS_EVERY: Duration = Duration::from_millis(100);
const FAULT_EVERY: Duration = Duration::from_millis(100);

/// Shared by every accept thread, the fault watcher and the counters writer.
pub struct Relay {
    upstream_host: String,
    /// Kept from the first lookup that resolves, so a connection pays no lookup of its own.
    upstream_ips: OnceLock<Vec<IpAddr>>,
    targets: Vec<Target>,
    unexposed: Vec<Unexposed>,
    changed: AtomicBool,
}

/// One exposed `name:port`.
struct Target {
    target: String,
    /// The route's `address:port`, the one a client of this target dials.
    at: SocketAddrV4,
    upstream: Vec<u16>,
    next: AtomicUsize,
    counts: Counts,
    fault: Mutex<Fault>,
}

/// One lock over the fault and the connections it cuts, so a connection is either in `open` when
/// a `down` comes or sees the `down` when it starts.
#[derive(Default)]
struct Fault {
    mode: Option<FaultSwitchMode>,
    open: Vec<Arc<Conn>>,
}

/// A relayed connection, shared with the fault watcher, which may cut it from its own thread.
struct Conn {
    client: TcpStream,
    upstream: OnceLock<TcpStream>,
    cut: AtomicBool,
}

#[derive(Default)]
struct Counts {
    connections: AtomicU64,
    open: AtomicU64,
    bytes_out: AtomicU64,
    bytes_in: AtomicU64,
    drops: AtomicU64,
    http: Mutex<Option<[u64; 5]>>,
}

struct Unexposed {
    name: String,
    address: Ipv4Addr,
    attempts: AtomicU64,
}

#[derive(Clone, Copy)]
enum Dir {
    Out,
    In,
}

struct Piped {
    bytes: u64,
    /// The source closed, rather than the other side refusing a write.
    by_source: bool,
}

impl Relay {
    /// The counters list the targets in the order of `routes`.
    pub fn new(upstream_host: &str, routes: &[Route], unexposed: &[UnexposedName]) -> Arc<Relay> {
        let targets = routes
            .iter()
            .map(|route| Target {
                target: format!("{}:{}", route.name, route.port),
                at: SocketAddrV4::new(route.address, route.port),
                upstream: route.upstream.clone(),
                next: AtomicUsize::new(0),
                counts: Counts::default(),
                fault: Mutex::default(),
            })
            .collect();
        let unexposed = unexposed
            .iter()
            .map(|name| Unexposed {
                name: name.name.clone(),
                address: name.address,
                attempts: AtomicU64::new(0),
            })
            .collect();
        Arc::new(Relay {
            upstream_host: upstream_host.to_owned(),
            upstream_ips: OnceLock::new(),
            targets,
            unexposed,
            // The file exists from the start, so a guest nobody connected from still has counters.
            changed: AtomicBool::new(true),
        })
    }

    /// Relays a connection, in its own thread, to the target at the address its client dialed. A
    /// dial to a name without `expose` is counted and closed, one to no target at all just closed.
    pub fn serve(self: &Arc<Relay>, dialed: SocketAddrV4, client: TcpStream) {
        if let Some(index) = self.targets.iter().position(|target| target.at == dialed) {
            let relay = Arc::clone(self);
            // `thread::spawn` panics in a guest out of threads, and would end the accept loop.
            let spawned =
                thread::Builder::new().spawn(move || relay.relay(&relay.targets[index], client));
            if let Err(error) = spawned {
                eprintln!("stand-guest net: closed a connection to {dialed}: no thread: {error}");
            }
        } else if let Some(name) = self
            .unexposed
            .iter()
            .find(|name| name.address == *dialed.ip())
        {
            name.attempts.fetch_add(1, Ordering::Relaxed);
            self.mark();
        }
    }

    /// Rewrites `path` on change, at most once per `COUNTERS_EVERY`, until `stop` is set; a change
    /// just before the stop is written on the way out. A failed write is tried again next round
    /// and reported on stderr once: the adapter waits for the file and would see only a timeout.
    pub fn write_counters(&self, path: &Path, stop: &AtomicBool) {
        let mut problem = Problem::default();
        loop {
            let stopping = stop.load(Ordering::Acquire);
            if self.changed.swap(false, Ordering::AcqRel) {
                match self.snapshot().write(path) {
                    Ok(()) => problem.gone(),
                    Err(error) => {
                        self.mark();
                        problem.show(format!("write {}: {error}", path.display()));
                    }
                }
            }
            if stopping {
                return;
            }
            thread::sleep(COUNTERS_EVERY);
        }
    }

    /// Reads `path` every `FAULT_EVERY` from now on and applies the switch in it; while there is
    /// no file no target has a fault. A file that cannot be read or parsed leaves the faults as
    /// they are and is reported on stderr once.
    pub fn watch_fault(self: &Arc<Relay>, path: PathBuf) {
        let relay = Arc::clone(self);
        thread::spawn(move || {
            let mut problem = Problem::default();
            loop {
                match read_fault(&path) {
                    Ok(switch) => {
                        relay.apply_fault(switch.as_ref());
                        problem.gone();
                    }
                    Err(why) => problem.show(why),
                }
                thread::sleep(FAULT_EVERY);
            }
        });
    }

    fn apply_fault(&self, switch: Option<&FaultSwitch>) {
        for target in &self.targets {
            let mode = switch
                .filter(|switch| switch.target == target.target)
                .map(|switch| switch.mode);
            target.set_fault(mode);
        }
    }

    fn relay(&self, target: &Target, client: TcpStream) {
        let accepted = Instant::now();
        let counts = &target.counts;
        counts.connections.fetch_add(1, Ordering::Relaxed);
        counts.open.fetch_add(1, Ordering::Relaxed);
        self.mark();
        let conn = Arc::new(Conn {
            client,
            upstream: OnceLock::new(),
            cut: AtomicBool::new(false),
        });
        let fault = target.admit(&conn);
        let _open = Open {
            relay: self,
            target,
            conn: &conn,
        };

        if fault == Some(FaultSwitchMode::Down) {
            conn.cut();
            return;
        }
        let Some(upstream) = self.connect(target) else {
            counts.drops.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let connected = Instant::now();
        let Some(upstream) = target.attach(&conn, upstream) else {
            return;
        };
        // Neither the client's bytes nor an upstream's greeting pass before the delay.
        if let Some(FaultSwitchMode::Slow { delay_ms }) = fault {
            thread::sleep(Duration::from_millis(delay_ms).saturating_sub(accepted.elapsed()));
        }
        let client = &conn.client;
        let sniff = Mutex::new(HttpSniff::default());
        let client_closed = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| {
                if self
                    .pipe(counts, client, upstream, Dir::Out, &sniff)
                    .by_source
                {
                    client_closed.store(true, Ordering::Release);
                }
                let _ = upstream.shutdown(Shutdown::Write);
            });
            let reply = self.pipe(counts, upstream, client, Dir::In, &sniff);
            // A cut connection ends with the reset of its close, and is no drop.
            if conn.is_cut() {
                return;
            }
            // The upstream closed soon, before any reply and before the client: how the network
            // limit of microsandbox shows from inside a guest.
            if reply.by_source
                && reply.bytes == 0
                && !client_closed.load(Ordering::Acquire)
                && connected.elapsed() <= DROP_WITHIN
            {
                counts.drops.fetch_add(1, Ordering::Relaxed);
                self.mark();
            }
            let _ = client.shutdown(Shutdown::Write);
        });
    }

    /// The next replica in turn; one that refuses passes the connection to the one after it.
    fn connect(&self, target: &Target) -> Option<TcpStream> {
        let ips = self.upstream_ips()?;
        let start = target.next.fetch_add(1, Ordering::Relaxed);
        let count = target.upstream.len();
        (0..count).find_map(|step| {
            let port = target.upstream[start.wrapping_add(step) % count];
            ips.iter().find_map(|&ip| {
                TcpStream::connect_timeout(&SocketAddr::new(ip, port), CONNECT_TIMEOUT).ok()
            })
        })
    }

    /// `None` while `upstream_host` does not resolve; the next connection looks it up again.
    fn upstream_ips(&self) -> Option<&[IpAddr]> {
        if let Some(ips) = self.upstream_ips.get() {
            return Some(ips);
        }
        let ips: Vec<IpAddr> = (self.upstream_host.as_str(), 0)
            .to_socket_addrs()
            .ok()?
            .map(|address| address.ip())
            .collect();
        (!ips.is_empty()).then(|| self.upstream_ips.get_or_init(|| ips).as_slice())
    }

    fn pipe(
        &self,
        counts: &Counts,
        mut from: &TcpStream,
        mut to: &TcpStream,
        dir: Dir,
        sniff: &Mutex<HttpSniff>,
    ) -> Piped {
        let counter = match dir {
            Dir::Out => &counts.bytes_out,
            Dir::In => &counts.bytes_in,
        };
        let mut buf = [0u8; 16 * 1024];
        let mut bytes = 0;
        loop {
            let n = match from.read(&mut buf) {
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Ok(0) | Err(_) => {
                    return Piped {
                        bytes,
                        by_source: true,
                    };
                }
                Ok(n) => n,
            };
            // Before the bytes go on, so a request's method is known before its response can come.
            self.sniff(counts, sniff, dir, &buf[..n]);
            if to.write_all(&buf[..n]).is_err() {
                return Piped {
                    bytes,
                    by_source: false,
                };
            }
            bytes += n as u64;
            counter.fetch_add(n as u64, Ordering::Relaxed);
            self.mark();
        }
    }

    fn sniff(&self, counts: &Counts, sniff: &Mutex<HttpSniff>, dir: Dir, bytes: &[u8]) {
        let mut sniff = sniff.lock().expect("sniff lock");
        let was_http = sniff.is_http();
        let statuses = match dir {
            Dir::Out => {
                sniff.feed_request(bytes);
                Vec::new()
            }
            Dir::In => sniff.feed_response(bytes),
        };
        // The target's lock, shared by its connections, only when there is something to add.
        if sniff.is_http() && (!was_http || !statuses.is_empty()) {
            let mut http = counts.http.lock().expect("http lock");
            let classes = http.get_or_insert([0; 5]);
            for status in statuses {
                classes[usize::from(status / 100) - 1] += 1;
            }
        }
    }

    fn mark(&self) {
        self.changed.store(true, Ordering::Release);
    }

    fn snapshot(&self) -> Counters {
        Counters {
            targets: self
                .targets
                .iter()
                .map(|t| TargetCounters {
                    target: t.target.clone(),
                    connections: t.counts.connections.load(Ordering::Relaxed),
                    open: t.counts.open.load(Ordering::Relaxed),
                    bytes_out: t.counts.bytes_out.load(Ordering::Relaxed),
                    bytes_in: t.counts.bytes_in.load(Ordering::Relaxed),
                    http: *t.counts.http.lock().expect("http lock"),
                    drops: t.counts.drops.load(Ordering::Relaxed),
                })
                .collect(),
            unexposed: self
                .unexposed
                .iter()
                .map(|u| UnexposedCount {
                    name: u.name.clone(),
                    attempts: u.attempts.load(Ordering::Relaxed),
                })
                .collect(),
        }
    }
}

/// A problem of a loop that runs every 100 ms goes to stderr once, until it changes or goes.
#[derive(Default)]
struct Problem(Option<String>);

impl Problem {
    fn show(&mut self, problem: String) {
        if self.0.as_ref() != Some(&problem) {
            eprintln!("stand-guest net: {problem}");
            self.0 = Some(problem);
        }
    }

    fn gone(&mut self) {
        self.0 = None;
    }
}

fn read_fault(path: &Path) -> Result<Option<FaultSwitch>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    FaultSwitch::parse(&text)
        .map(Some)
        .map_err(|error| format!("{}: {error}", path.display()))
}

impl Target {
    fn set_fault(&self, mode: Option<FaultSwitchMode>) {
        let mut fault = self.fault.lock().expect("fault lock");
        if mode == Some(FaultSwitchMode::Down) && fault.mode != mode {
            for conn in &fault.open {
                conn.cut();
            }
        }
        fault.mode = mode;
    }

    /// Puts a new connection where a `down` reaches it; returns the fault it starts under.
    fn admit(&self, conn: &Arc<Conn>) -> Option<FaultSwitchMode> {
        let mut fault = self.fault.lock().expect("fault lock");
        fault.open.push(Arc::clone(conn));
        fault.mode
    }

    /// `None` when a `down` cut the connection while it dialed the upstream.
    fn attach<'c>(&self, conn: &'c Conn, upstream: TcpStream) -> Option<&'c TcpStream> {
        let _fault = self.fault.lock().expect("fault lock");
        if conn.is_cut() {
            return None;
        }
        Some(conn.upstream.get_or_init(|| upstream))
    }

    fn leave(&self, conn: &Arc<Conn>) {
        let mut fault = self.fault.lock().expect("fault lock");
        fault.open.retain(|open| !Arc::ptr_eq(open, conn));
    }
}

impl Conn {
    /// Wakes both pipes and leaves the client socket to close with `SO_LINGER` 0, so the client
    /// gets a reset and no FIN. A pipe blocked writing to a client that does not read waits until
    /// it reads.
    fn cut(&self) {
        self.cut.store(true, Ordering::Release);
        let linger = libc::linger {
            l_onoff: 1,
            l_linger: 0,
        };
        // SAFETY: the option value is a `linger` of the length given.
        unsafe {
            libc::setsockopt(
                self.client.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_LINGER,
                (&raw const linger).cast(),
                size_of::<libc::linger>() as libc::socklen_t,
            )
        };
        let _ = self.client.shutdown(Shutdown::Read);
        if let Some(upstream) = self.upstream.get() {
            let _ = upstream.shutdown(Shutdown::Both);
        }
    }

    fn is_cut(&self) -> bool {
        self.cut.load(Ordering::Acquire)
    }
}

/// Takes the connection off `open` and out of the fault's reach however `relay` returns; its
/// sockets close after that, with the last `Arc`.
struct Open<'a> {
    relay: &'a Relay,
    target: &'a Target,
    conn: &'a Arc<Conn>,
}

impl Drop for Open<'_> {
    fn drop(&mut self) {
        self.target.leave(self.conn);
        self.target.counts.open.fetch_sub(1, Ordering::Relaxed);
        self.relay.mark();
    }
}
