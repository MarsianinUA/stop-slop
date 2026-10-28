//! The relay, shared by both entries. A connection to an exposed name goes round-robin to one of
//! the upstream host ports, and its bytes are counted as they pass.

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use guest::{Counters, Route, TargetCounters, UnexposedCount, UnexposedName};

use crate::sniff::HttpSniff;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const COUNTERS_EVERY: Duration = Duration::from_millis(100);

/// Shared by every accept thread and the counters writer.
pub struct Relay {
    upstream_host: String,
    targets: Vec<Target>,
    unexposed: Vec<Unexposed>,
    changed: AtomicBool,
}

/// One exposed `name:port`.
struct Target {
    target: String,
    upstream: Vec<u16>,
    next: AtomicUsize,
    counts: Counts,
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
    /// Target `i` is `routes[i]`.
    pub fn new(upstream_host: &str, routes: &[Route], unexposed: &[UnexposedName]) -> Arc<Relay> {
        let targets = routes
            .iter()
            .map(|route| Target {
                target: format!("{}:{}", route.name, route.port),
                upstream: route.upstream.clone(),
                next: AtomicUsize::new(0),
                counts: Counts::default(),
            })
            .collect();
        let unexposed = unexposed
            .iter()
            .map(|name| Unexposed {
                name: name.name.clone(),
                attempts: AtomicU64::new(0),
            })
            .collect();
        Arc::new(Relay {
            upstream_host: upstream_host.to_owned(),
            targets,
            unexposed,
            // The file exists from the start, so a guest nobody connected from still has counters.
            changed: AtomicBool::new(true),
        })
    }

    /// Relays a connection that reached target `index`, in its own thread.
    pub fn serve(self: &Arc<Relay>, index: usize, client: TcpStream) {
        let relay = Arc::clone(self);
        thread::spawn(move || relay.relay(&relay.targets[index], client));
    }

    /// Rewrites `path` on change, at most once per `COUNTERS_EVERY`, until `stop` is set; a change
    /// just before the stop is written on the way out.
    pub fn write_counters(&self, path: &Path, stop: &AtomicBool) {
        loop {
            let stopping = stop.load(Ordering::Acquire);
            if self.changed.swap(false, Ordering::AcqRel) && self.snapshot().write(path).is_err() {
                self.mark();
            }
            if stopping {
                return;
            }
            thread::sleep(COUNTERS_EVERY);
        }
    }

    fn relay(&self, target: &Target, client: TcpStream) {
        let counts = &target.counts;
        counts.connections.fetch_add(1, Ordering::Relaxed);
        counts.open.fetch_add(1, Ordering::Relaxed);
        self.mark();
        let _open = Open {
            relay: self,
            counts,
        };

        let Some(upstream) = self.connect(target) else {
            counts.drops.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let sniff = Mutex::new(HttpSniff::default());
        let client_closed = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| {
                if self
                    .pipe(counts, &client, &upstream, Dir::Out, &sniff)
                    .by_source
                {
                    client_closed.store(true, Ordering::Release);
                }
                let _ = upstream.shutdown(Shutdown::Write);
            });
            let reply = self.pipe(counts, &upstream, &client, Dir::In, &sniff);
            // The upstream closed before any reply and before the client: how the network limit
            // of microsandbox shows from inside a guest.
            if reply.by_source && reply.bytes == 0 && !client_closed.load(Ordering::Acquire) {
                counts.drops.fetch_add(1, Ordering::Relaxed);
                self.mark();
            }
            let _ = client.shutdown(Shutdown::Write);
        });
    }

    /// The next replica in turn; one that refuses passes the connection to the one after it.
    fn connect(&self, target: &Target) -> Option<TcpStream> {
        let start = target.next.fetch_add(1, Ordering::Relaxed);
        let count = target.upstream.len();
        (0..count).find_map(|step| {
            let port = target.upstream[start.wrapping_add(step) % count];
            let addresses = (self.upstream_host.as_str(), port).to_socket_addrs().ok()?;
            addresses
                .into_iter()
                .find_map(|address| TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).ok())
        })
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
        let statuses = match dir {
            Dir::Out => {
                sniff.feed_request(bytes);
                Vec::new()
            }
            Dir::In => sniff.feed_response(bytes),
        };
        if sniff.is_http() {
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

/// Takes the connection off `open` however `relay` returns.
struct Open<'a> {
    relay: &'a Relay,
    counts: &'a Counts,
}

impl Drop for Open<'_> {
    fn drop(&mut self) {
        self.counts.open.fetch_sub(1, Ordering::Relaxed);
        self.relay.mark();
    }
}
