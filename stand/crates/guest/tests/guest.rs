//! `stand-guest` driven on the host. The forwarder runs in `--direct` mode, so no root or netfilter
//! is needed: a listener binds each route's loopback `address:port`, and the relay is the same code
//! the guest uses behind REDIRECT. macOS has only `127.0.0.1` on loopback, so the routes share that
//! address and differ by port. The forwarder is Unix only; the arguments, the files and the Windows
//! stub are checked everywhere.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use guest::{
    Command, Counters, Entry, ExecArgs, FaultSwitch, FaultSwitchMode, GuestError, Route, Routes,
    RunArgs, StopArgs, StopReport, TargetCounters, TrimArgs, UnexposedCount, UnexposedName,
};
use serde_json::json;

const BIN: &str = env!("CARGO_BIN_EXE_stand-guest");

#[test]
fn argument_round_trip_for_every_subcommand() {
    let exec = ExecArgs {
        out: "/stand/out/probe.orders.jsonl".into(),
        deadline: Duration::from_millis(30_000),
        user: "postgres".into(),
        argv: vec!["psql".into(), "-c".into(), "select 1".into()],
    };
    let cases = [
        Command::Net(Routes {
            upstream_host: "host.microsandbox.internal".into(),
            entry: Entry::Redirect,
            routes: vec![
                Route {
                    name: "api".into(),
                    port: 3000,
                    address: Ipv4Addr::new(127, 0, 0, 2),
                    upstream: vec![40001, 40002],
                },
                Route {
                    name: "db".into(),
                    port: 5432,
                    address: Ipv4Addr::new(127, 0, 0, 3),
                    upstream: vec![40003],
                },
            ],
            unexposed: vec![UnexposedName {
                name: "cache".into(),
                address: Ipv4Addr::new(127, 0, 0, 4),
            }],
            counters: PathBuf::from("/stand/net/counters.json"),
            fault: PathBuf::from("/stand/net/fault.json"),
        }),
        Command::Net(Routes {
            upstream_host: "127.0.0.1".into(),
            entry: Entry::Direct,
            routes: vec![],
            unexposed: vec![],
            counters: "counters.json".into(),
            fault: "fault.json".into(),
        }),
        Command::Run(RunArgs {
            status: "/stand/net/status.json".into(),
            command: vec!["node".into(), "server.js".into()],
        }),
        Command::Stop(StopArgs {
            status: "/stand/net/status.json".into(),
            signal: "SIGTERM".into(),
            grace: Duration::from_millis(10_000),
        }),
        Command::Exec(exec.clone()),
        Command::Exec(ExecArgs {
            user: String::new(),
            argv: vec!["sh".into(), "-c".into(), "-- x".into()],
            ..exec
        }),
        Command::Trim(TrimArgs {
            mounts: vec!["/w".into(), "/data".into()],
        }),
    ];
    for command in cases {
        let parsed = Command::parse(&command.to_args()).expect("the arguments parse");
        assert_eq!(parsed, command, "{command:?} did not round-trip");
    }
}

#[test]
fn bad_arguments_are_rejected() {
    let cases = [
        "",
        "dance",
        "stop",
        "net --upstream",
        "net --upstream h --counters c --fault f",
        "net --upstream h --counters c --fault f --direct --redirect",
        "net --upstream h --counters c --fault f --direct --route api:3000@127.0.0.2",
        "stop --status s --signal SIGTERM --grace soon",
        "stop --status s --status t --signal SIGTERM --grace 1",
        "run --status s",
        "trim --mount /w -- fstrim",
    ];
    for case in cases {
        let args: Vec<String> = case.split_whitespace().map(String::from).collect();
        assert!(
            matches!(Command::parse(&args), Err(GuestError::Args(_))),
            "{case:?} was accepted"
        );
    }
}

/// A released `stand-guest` and a newer adapter read each other's files, so their shapes are fixed.
#[test]
fn the_files_keep_their_json_shape() {
    let down = FaultSwitch {
        target: "db:5432".into(),
        mode: FaultSwitchMode::Down,
    };
    let slow = FaultSwitch {
        target: "db:5432".into(),
        mode: FaultSwitchMode::Slow { delay_ms: 2000 },
    };
    let stopped = StopReport {
        after_ms: 120,
        killed: true,
        code: None,
    };
    let counters = Counters {
        targets: vec![TargetCounters {
            target: "api:3000".into(),
            connections: 3,
            open: 1,
            bytes_out: 10,
            bytes_in: 20,
            http: Some([0, 2, 0, 0, 1]),
            drops: 0,
        }],
        unexposed: vec![UnexposedCount {
            name: "cache".into(),
            attempts: 1,
        }],
    };
    let down_json = json!({"target": "db:5432", "mode": "down"});
    let slow_json = json!({"target": "db:5432", "mode": "slow", "delayMs": 2000});
    let stopped_json = json!({"afterMs": 120, "killed": true, "code": null});
    let counters_json = json!({
        "targets": [{"target": "api:3000", "connections": 3, "open": 1, "bytesOut": 10, "bytesIn": 20, "http": [0, 2, 0, 0, 1], "drops": 0}],
        "unexposed": [{"name": "cache", "attempts": 1}],
    });

    assert_eq!(serde_json::to_value(&down).unwrap(), down_json);
    assert_eq!(serde_json::to_value(&slow).unwrap(), slow_json);
    assert_eq!(serde_json::to_value(stopped).unwrap(), stopped_json);
    assert_eq!(serde_json::to_value(&counters).unwrap(), counters_json);
    assert_eq!(FaultSwitch::parse(&down_json.to_string()).unwrap(), down);
    assert_eq!(FaultSwitch::parse(&slow_json.to_string()).unwrap(), slow);
    assert_eq!(
        StopReport::parse(&format!("{stopped_json}\n")).unwrap(),
        stopped
    );
    assert_eq!(
        Counters::parse(&counters_json.to_string()).unwrap(),
        counters
    );
}

/// On Windows the binary is the `cfg(not(unix))` stub, which exits non-zero; elsewhere an unknown
/// subcommand does.
#[test]
fn the_binary_runs() {
    let status = std::process::Command::new(BIN)
        .arg("--help-is-not-a-command")
        .status()
        .expect("run stand-guest");
    assert_eq!(status.code(), Some(125));
}

#[cfg(unix)]
mod forwarder {
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::path::Path;
    use std::process::{Child, Command as Process};
    use std::thread;
    use std::time::Instant;

    use super::*;

    const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;

    /// Reads each request to its end, writes `reply` and closes.
    fn upstream(reply: &'static [u8]) -> u16 {
        let listener = TcpListener::bind((LOOPBACK, 0)).unwrap();
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

    /// Closes each connection at once, before any reply.
    fn closing_upstream() -> u16 {
        let listener = TcpListener::bind((LOOPBACK, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || listener.incoming().for_each(drop));
        port
    }

    fn free_port() -> u16 {
        TcpListener::bind((LOOPBACK, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// A port nothing listens on. The client end of a connection holds it, so while the returned
    /// sockets live no other socket can take it, as one could take a port from `free_port`.
    fn refusing_port() -> (u16, (TcpListener, TcpStream)) {
        let listener = TcpListener::bind((LOOPBACK, 0)).unwrap();
        let held = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        (held.local_addr().unwrap().port(), (listener, held))
    }

    fn route(name: &str, upstream: &[u16]) -> Route {
        Route {
            name: name.into(),
            port: free_port(),
            address: LOOPBACK,
            upstream: upstream.to_vec(),
        }
    }

    struct Forwarder {
        child: Child,
        counters: PathBuf,
        ports: Vec<u16>,
        _temp: tempfile::TempDir,
    }

    impl Forwarder {
        /// Returns once the first counters file is there, which `net` writes after binding. Another
        /// socket can take a port from `free_port` before `net` binds it; `net` then exits and
        /// starts again on new ports.
        fn start(mut routes: Vec<Route>) -> Forwarder {
            let temp = tempfile::tempdir().unwrap();
            let counters = temp.path().join("counters.json");
            for _ in 0..5 {
                let command = Command::Net(Routes {
                    upstream_host: LOOPBACK.to_string(),
                    entry: Entry::Direct,
                    routes: routes.clone(),
                    unexposed: vec![],
                    counters: counters.clone(),
                    fault: temp.path().join("fault.json"),
                });
                let mut child = Process::new(BIN).args(command.to_args()).spawn().unwrap();
                if bound(&mut child, &counters) {
                    return Forwarder {
                        child,
                        counters,
                        ports: routes.iter().map(|route| route.port).collect(),
                        _temp: temp,
                    };
                }
                for route in &mut routes {
                    route.port = free_port();
                }
            }
            panic!("net exited five times before it bound its ports");
        }

        fn connect(&self, target: usize) -> TcpStream {
            TcpStream::connect((LOOPBACK, self.ports[target])).unwrap()
        }

        /// Sends `body`, closes the sending side and reads the reply to its end. A target that is
        /// down resets the connection, so the errors are part of the answer, not a failure.
        fn request(&self, target: usize, body: &[u8]) -> Vec<u8> {
            let mut stream = self.connect(target);
            let _ = stream.write_all(body);
            let _ = stream.shutdown(Shutdown::Write);
            let mut reply = Vec::new();
            let _ = stream.read_to_end(&mut reply);
            reply
        }

        /// Every read of the file must parse whole.
        fn counters_until(&self, done: impl Fn(&Counters) -> bool) -> Counters {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut last = None;
            while Instant::now() < deadline {
                if let Ok(text) = std::fs::read_to_string(&self.counters) {
                    let counters = Counters::parse(&text).expect("the counters file parses whole");
                    if done(&counters) {
                        return counters;
                    }
                    last = Some(counters);
                }
                thread::sleep(Duration::from_millis(10));
            }
            panic!("the counters never got there; the last ones: {last:?}");
        }
    }

    /// Whether `net` wrote its first counters, rather than exit.
    fn bound(child: &mut Child, counters: &Path) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if counters.exists() {
                return true;
            }
            if child.try_wait().unwrap().is_some() {
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("net neither wrote its counters nor exited");
    }

    impl Drop for Forwarder {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn settled(counters: &Counters) -> bool {
        counters
            .targets
            .iter()
            .all(|target| target.connections == 1 && target.open == 0)
    }

    #[test]
    fn connections_open_and_bytes_are_counted_per_target() {
        let fwd = Forwarder::start(vec![
            route("api", &[upstream(b"pong")]),
            route("db", &[upstream(b"")]),
        ]);
        let mut client = fwd.connect(0);
        client.write_all(b"ping").unwrap();
        let during = fwd.counters_until(|c| c.targets[0].bytes_out == 4);
        assert_eq!(during.targets[0].connections, 1);
        assert_eq!(during.targets[0].open, 1);

        client.shutdown(Shutdown::Write).unwrap();
        let mut reply = Vec::new();
        client.read_to_end(&mut reply).unwrap();
        assert_eq!(reply, b"pong");
        let after = fwd.counters_until(|c| c.targets[0].open == 0 && c.targets[0].bytes_in == 4);
        let counted = |name: &str, port: u16, connections: u64, bytes: u64| TargetCounters {
            target: format!("{name}:{port}"),
            connections,
            open: 0,
            bytes_out: bytes,
            bytes_in: bytes,
            http: None,
            drops: 0,
        };
        assert_eq!(
            after.targets,
            [
                counted("api", fwd.ports[0], 1, 4),
                counted("db", fwd.ports[1], 0, 0)
            ]
        );
    }

    #[test]
    fn the_last_change_is_written_before_exit() {
        let mut fwd = Forwarder::start(vec![route("api", &[upstream(b"pong")])]);
        assert_eq!(fwd.request(0, b"ping"), b"pong");
        // At once, before the writer's next round, so only the write on the way out has the change.
        unsafe { libc::kill(fwd.child.id() as libc::pid_t, libc::SIGTERM) };
        assert!(fwd.child.wait().unwrap().success());
        let counters = Counters::parse(&std::fs::read_to_string(&fwd.counters).unwrap()).unwrap();
        assert_eq!(counters.targets[0].connections, 1);
        assert_eq!(counters.targets[0].bytes_in, 4);
    }

    #[test]
    fn status_classes_come_from_http_responses_and_not_from_tls() {
        let responses = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok\
            HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n";
        let fwd = Forwarder::start(vec![
            route("api", &[upstream(responses)]),
            route("tls", &[upstream(b"\x16\x03\x03\x00\x02hi")]),
        ]);
        // Two requests on one kept-alive connection.
        fwd.request(
            0,
            b"GET / HTTP/1.1\r\nHost: api\r\n\r\nGET /health HTTP/1.1\r\nHost: api\r\n\r\n",
        );
        fwd.request(1, b"\x16\x03\x01\x00\x05hello");
        let counters = fwd.counters_until(settled);
        assert_eq!(counters.targets[0].http, Some([0, 1, 0, 0, 1]));
        assert_eq!(counters.targets[1].http, None);
    }

    #[test]
    fn a_target_that_refuses_or_closes_first_counts_a_drop() {
        let (refusing, _held) = refusing_port();
        let fwd = Forwarder::start(vec![
            route("refused", &[refusing]),
            route("closing", &[closing_upstream()]),
            route("quiet", &[upstream(b"")]),
        ]);
        assert_eq!(fwd.request(0, b"ping"), b"");
        // The client sends nothing and waits, so the upstream closes first.
        let _ = fwd.connect(1).read_to_end(&mut Vec::new());
        // The client closes first, and the upstream answers nothing: not a drop.
        assert_eq!(fwd.request(2, b"ping"), b"");
        let drops: Vec<u64> = fwd
            .counters_until(settled)
            .targets
            .iter()
            .map(|target| target.drops)
            .collect();
        assert_eq!(drops, [1, 1, 0]);
    }

    #[test]
    fn two_replicas_take_turns() {
        let fwd = Forwarder::start(vec![route("api", &[upstream(b"a"), upstream(b"b")])]);
        let replies: Vec<Vec<u8>> = (0..3).map(|_| fwd.request(0, b"ping")).collect();
        assert_eq!(replies, [b"a", b"b", b"a"]);
    }
}
