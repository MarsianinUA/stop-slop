//! `net --redirect` against the kernel's netfilter, Linux only. Rules need CAP_NET_ADMIN, so each
//! test runs itself again in a new user and network namespace (`unshare`): there it is root over
//! an empty netfilter, with only `lo`. Where no such namespace can be made (no `unshare`, a kernel
//! or AppArmor that forbids it) the test says why on stderr and passes, unless
//! `STAND_GUEST_NF_REQUIRED` is set, as on CI: then it fails.

#![cfg(target_os = "linux")]

use std::io::{self, ErrorKind, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command as Process, Output};
use std::thread;
use std::time::{Duration, Instant};

use guest::{Command, Counters, Entry, PROBE_GID, Route, Routes, UnexposedName};

const BIN: &str = env!("CARGO_BIN_EXE_stand-guest");
/// Set to the test's name in the run inside the namespace.
const INNER: &str = "STAND_GUEST_NF_INNER";
const REQUIRED: &str = "STAND_GUEST_NF_REQUIRED";

const API: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 2);
const DB: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 3);
const CACHE: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 4);

#[test]
fn redirect_takes_a_name_connection_to_the_target_it_dialed() {
    in_namespace(
        "redirect_takes_a_name_connection_to_the_target_it_dialed",
        0,
        || {
            let mut fwd = Forwarder::start(
                vec![
                    route("api", 3000, API, upstream(b"api")),
                    route("api", 9229, API, upstream(b"debug")),
                    route("db", 5432, DB, upstream(b"db")),
                ],
                vec![UnexposedName {
                    name: "cache".into(),
                    address: CACHE,
                }],
            );

            assert_eq!(request((API, 3000)), b"api");
            assert_eq!(request((API, 9229)), b"debug");
            assert_eq!(request((DB, 5432)), b"db");
            // Both reach the forwarder, which counts the name without `expose` and closes both.
            assert_eq!(request((CACHE, 6379)), b"");
            assert_eq!(request((API, 4000)), b"");
            let counters = fwd.counters_until(|c| {
                c.unexposed[0].attempts == 1 && c.targets.iter().all(|t| t.open == 0)
            });
            let connections: Vec<u64> = counters.targets.iter().map(|t| t.connections).collect();
            assert_eq!(connections, [1, 1, 1]);

            // A second `net` would add a rule to its own port behind the first one.
            let mut second = Process::new(BIN).args(&fwd.args).spawn().unwrap();
            assert_eq!(exit_within(&mut second, Duration::from_secs(5)), Some(125));
        },
    );
}

#[test]
fn a_probe_reaches_loopback_and_nothing_else() {
    in_namespace(
        "a_probe_reaches_loopback_and_nothing_else",
        PROBE_GID,
        || {
            use Dial::{Connected, Refused};
            assert_eq!(
                dial_out().0,
                [Connected, Refused, Refused, Connected, Refused]
            )
        },
    );
}

#[test]
fn another_group_reaches_an_outside_address_and_the_names() {
    in_namespace(
        "another_group_reaches_an_outside_address_and_the_names",
        0,
        || {
            let (dials, _fwd) = dial_out();
            assert_eq!(dials, [Dial::Connected; 5]);
            assert_eq!(request((API, 3000)), b"api");
        },
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dial {
    Connected,
    Refused,
}

/// Dials loopback, an address outside `127.0.0.0/8` and a name, then the IPv6 loopback and an
/// address beside it. The outside addresses are put on `lo` too, so nothing but the rules can
/// refuse them.
fn dial_out() -> ([Dial; 5], Forwarder) {
    let outside = Ipv4Addr::new(10, 1, 1, 1);
    let outside6 = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1);
    ip(&["address", "add", &format!("{outside}/32"), "dev", "lo"]);
    ip(&[
        "-6",
        "address",
        "add",
        &format!("{outside6}/128"),
        "dev",
        "lo",
    ]);
    // Both families: `bindv6only` is off in a new namespace.
    let local = TcpListener::bind((Ipv6Addr::UNSPECIFIED, 0)).unwrap();
    let port = local.local_addr().unwrap().port();
    let fwd = Forwarder::start(vec![route("api", 3000, API, upstream(b"api"))], vec![]);
    let to: [SocketAddr; 5] = [
        (Ipv4Addr::LOCALHOST, port).into(),
        (outside, port).into(),
        (API, 3000).into(),
        (Ipv6Addr::LOCALHOST, port).into(),
        (outside6, port).into(),
    ];
    let dials = to.map(
        |to| match TcpStream::connect_timeout(&to, Duration::from_secs(5)) {
            Ok(_) => Dial::Connected,
            Err(error) if error.kind() == ErrorKind::ConnectionRefused => Dial::Refused,
            Err(error) => panic!("dial {to}: {error}"),
        },
    );
    (dials, fwd)
}

/// Runs `body` as root of a new user and network namespace, where every process has group `gid`.
fn in_namespace(name: &str, gid: u32, body: fn()) {
    if std::env::var(INNER).as_deref() == Ok(name) {
        assert_eq!(unsafe { libc::getgid() }, gid);
        ip(&["link", "set", "lo", "up"]);
        body();
        return;
    }
    match unshare(gid).arg("true").output() {
        Ok(output) if output.status.success() => {}
        tried => return skip(name, tried),
    }
    let inner = unshare(gid)
        .arg(std::env::current_exe().unwrap())
        .args([name, "--exact", "--nocapture"])
        .env(INNER, name)
        .output()
        .expect("run the test again in the namespace");
    let stdout = String::from_utf8_lossy(&inner.stdout);
    print!("{stdout}");
    eprint!("{}", String::from_utf8_lossy(&inner.stderr));
    assert!(inner.status.success(), "{name} failed in the namespace");
    assert!(
        stdout.contains("1 passed"),
        "{name} did not run in the namespace"
    );
}

/// `-rn` maps the caller to root and group 0; another group needs `--map-group` (util-linux 2.38).
fn unshare(gid: u32) -> Process {
    let mut unshare = Process::new("unshare");
    if gid == 0 {
        unshare.arg("-rn");
    } else {
        unshare.args(["--map-user=0", &format!("--map-group={gid}"), "--net"]);
    }
    unshare
}

fn skip(name: &str, tried: io::Result<Output>) {
    let why = match tried {
        Ok(output) => String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        Err(error) => format!("unshare: {error}"),
    };
    let line = format!("{name}: no user and network namespace: {why}");
    assert!(std::env::var_os(REQUIRED).is_none(), "{line}");
    // Past the harness's capture, so the skip shows in a passing run.
    let _ = writeln!(io::stderr(), "skipped {line}");
}

fn ip(args: &[&str]) {
    let status = Process::new("ip").args(args).status().expect("run ip");
    assert!(status.success(), "ip {}", args.join(" "));
}

fn route(name: &str, port: u16, address: Ipv4Addr, upstream: u16) -> Route {
    Route {
        name: name.into(),
        port,
        address,
        upstream: vec![upstream],
    }
}

/// Reads each request to its end, answers `reply` and closes.
fn upstream(reply: &'static [u8]) -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            thread::spawn(move || {
                let _ = stream.read_to_end(&mut Vec::new());
                let _ = stream.write_all(reply);
            });
        }
    });
    port
}

/// A closed connection is part of the answer, so write and read errors are not failures.
fn request(to: (Ipv4Addr, u16)) -> Vec<u8> {
    let mut stream = TcpStream::connect(to).unwrap();
    let _ = stream.write_all(b"ping");
    let _ = stream.shutdown(Shutdown::Write);
    let mut reply = Vec::new();
    let _ = stream.read_to_end(&mut reply);
    reply
}

fn exit_within(child: &mut Child, limit: Duration) -> Option<i32> {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code();
        }
        thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("still running after {limit:?}");
}

struct Forwarder {
    child: Child,
    args: Vec<String>,
    counters: PathBuf,
    _temp: tempfile::TempDir,
}

impl Forwarder {
    /// Returns once the first counters file is there, which `net` writes after the rules are in.
    fn start(routes: Vec<Route>, unexposed: Vec<UnexposedName>) -> Forwarder {
        let temp = tempfile::tempdir().unwrap();
        let counters = temp.path().join("counters.json");
        let args = Command::Net(Routes {
            upstream_host: Ipv4Addr::LOCALHOST.to_string(),
            entry: Entry::Redirect,
            routes,
            unexposed,
            counters: counters.clone(),
            fault: temp.path().join("fault.json"),
        })
        .to_args();
        let mut forwarder = Forwarder {
            child: Process::new(BIN).args(&args).spawn().unwrap(),
            args,
            counters,
            _temp: temp,
        };
        forwarder.counters_until(|_| true);
        forwarder
    }

    fn counters_until(&mut self, done: impl Fn(&Counters) -> bool) -> Counters {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(text) = std::fs::read_to_string(&self.counters) {
                let counters = Counters::parse(&text).expect("the counters file parses whole");
                if done(&counters) {
                    return counters;
                }
            }
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!("net exited with {status}");
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("the counters never got there");
    }
}

impl Drop for Forwarder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
