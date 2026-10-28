//! `stand-guest` driven on the host. The forwarder runs in `--direct` mode, so no root or netfilter
//! is needed: a listener binds each route's loopback `address:port`, and the relay is the same code
//! the guest uses behind REDIRECT. macOS has only `127.0.0.1` on loopback, so the routes share that
//! address and differ by port. The forwarder, `run`, `stop` and `exec` are Unix only, `trim` is
//! Linux only; the arguments, the files and the Windows stub are checked everywhere.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use guest::{
    Command, Counters, Entry, ExecArgs, FaultSwitch, FaultSwitchMode, GuestError, Route, Routes,
    RunArgs, StopArgs, StopReport, TargetCounters, TrimArgs, UnexposedCount, UnexposedName,
};
use serde_json::json;

const BIN: &str = env!("CARGO_BIN_EXE_stand-guest");
#[cfg(unix)]
const LONG: Duration = Duration::from_secs(10);

#[cfg(unix)]
fn guest(command: &Command) -> std::process::Command {
    let mut process = std::process::Command::new(BIN);
    process.args(command.to_args());
    process
}

#[cfg(unix)]
fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + LONG;
    while !done() {
        assert!(std::time::Instant::now() < deadline, "{what} never came");
        std::thread::sleep(Duration::from_millis(10));
    }
}

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

#[cfg(unix)]
#[test]
fn a_file_is_written_past_a_link_at_its_temp_name() {
    let temp = tempfile::tempdir().unwrap();
    let victim = temp.path().join("victim");
    std::fs::write(&victim, "kept").unwrap();
    std::os::unix::fs::symlink(&victim, temp.path().join("counters.json.tmp")).unwrap();
    let path = temp.path().join("counters.json");
    let counters = Counters {
        targets: vec![],
        unexposed: vec![],
    };
    counters.write(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "kept");
    let written = Counters::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(written, counters);
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
    use std::io::{self, ErrorKind, Read, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::path::Path;
    use std::process::Child;
    use std::thread;
    use std::time::Instant;

    use super::*;

    const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;

    fn listen(serve: impl FnOnce(TcpListener) + Send + 'static) -> u16 {
        let listener = TcpListener::bind((LOOPBACK, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || serve(listener));
        port
    }

    /// Reads each request to its end, writes `reply` and closes.
    fn upstream(reply: &'static [u8]) -> u16 {
        listen(move |listener| {
            for mut stream in listener.incoming().flatten() {
                thread::spawn(move || {
                    let _ = stream.read_to_end(&mut Vec::new());
                    let _ = stream.write_all(reply);
                });
            }
        })
    }

    /// Writes back what it reads until the client closes, so a connection stays open between
    /// exchanges.
    fn echo_upstream() -> u16 {
        listen(|listener| {
            for stream in listener.incoming().flatten() {
                thread::spawn(move || io::copy(&mut &stream, &mut &stream));
            }
        })
    }

    fn closing_upstream(after: Duration) -> u16 {
        listen(move |listener| {
            for stream in listener.incoming().flatten() {
                thread::spawn(move || {
                    thread::sleep(after);
                    drop(stream);
                });
            }
        })
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
        fault: PathBuf,
        ports: Vec<u16>,
        /// `name:port` of each route, as the counters and the fault switch name it.
        targets: Vec<String>,
        _temp: tempfile::TempDir,
    }

    impl Forwarder {
        /// Returns once the first counters file is there, which `net` writes after binding. Another
        /// socket can take a port from `free_port` before `net` binds it; `net` then exits and
        /// starts again on new ports.
        fn start(mut routes: Vec<Route>) -> Forwarder {
            let temp = tempfile::tempdir().unwrap();
            let counters = temp.path().join("counters.json");
            let fault = temp.path().join("fault.json");
            for _ in 0..5 {
                let command = Command::Net(Routes {
                    upstream_host: LOOPBACK.to_string(),
                    entry: Entry::Direct,
                    routes: routes.clone(),
                    unexposed: vec![],
                    counters: counters.clone(),
                    fault: fault.clone(),
                });
                let mut child = guest(&command).spawn().unwrap();
                if bound(&mut child, &counters) {
                    return Forwarder {
                        child,
                        counters,
                        fault,
                        ports: routes.iter().map(|route| route.port).collect(),
                        targets: routes
                            .iter()
                            .map(|route| format!("{}:{}", route.name, route.port))
                            .collect(),
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
            let stream = TcpStream::connect((LOOPBACK, self.ports[target])).unwrap();
            stream.set_read_timeout(Some(LONG)).unwrap();
            stream
        }

        /// The time from the dial of `target` to the first byte of the echo of `ping`.
        fn first_byte(&self, target: usize) -> Duration {
            let started = Instant::now();
            let mut stream = self.connect(target);
            stream.write_all(b"ping").unwrap();
            stream.read_exact(&mut [0]).unwrap();
            started.elapsed()
        }

        fn switch(&self, target: usize, mode: FaultSwitchMode) {
            let target = self.targets[target].clone();
            FaultSwitch { target, mode }.write(&self.fault).unwrap();
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
            let deadline = Instant::now() + LONG;
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
        let deadline = Instant::now() + LONG;
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
    fn a_target_that_refuses_or_soon_closes_first_counts_a_drop() {
        let (refusing, _held) = refusing_port();
        let fwd = Forwarder::start(vec![
            route("refused", &[refusing]),
            route("closing", &[closing_upstream(Duration::ZERO)]),
            route("quiet", &[upstream(b"")]),
            route("idle", &[closing_upstream(Duration::from_secs(4))]),
        ]);
        assert_eq!(fwd.request(0, b"ping"), b"");
        // The client sends nothing and waits, so the upstream closes first.
        let _ = fwd.connect(1).read_to_end(&mut Vec::new());
        // The client closes first, and the upstream answers nothing: not a drop.
        assert_eq!(fwd.request(2, b"ping"), b"");
        // The upstream closes first, but long after the limit of microsandbox would have.
        let _ = fwd.connect(3).read_to_end(&mut Vec::new());
        let drops: Vec<u64> = fwd
            .counters_until(settled)
            .targets
            .iter()
            .map(|target| target.drops)
            .collect();
        assert_eq!(drops, [1, 1, 0, 0]);
    }

    #[test]
    fn two_replicas_take_turns() {
        let fwd = Forwarder::start(vec![route("api", &[upstream(b"a"), upstream(b"b")])]);
        let replies: Vec<Vec<u8>> = (0..3).map(|_| fwd.request(0, b"ping")).collect();
        assert_eq!(replies, [b"a", b"b", b"a"]);
    }

    #[test]
    fn a_down_fault_resets_the_open_connections_and_refuses_new_ones_while_the_file_is_there() {
        let fwd = Forwarder::start(vec![
            route("db", &[echo_upstream()]),
            route("api", &[echo_upstream()]),
        ]);
        let mut talked = fwd.connect(0);
        talked.write_all(b"ping").unwrap();
        talked.read_exact(&mut [0; 4]).unwrap();
        let quiet = fwd.connect(0);
        let mut other = fwd.connect(1);
        fwd.counters_until(|c| c.targets[0].open == 2 && c.targets[1].open == 1);

        fwd.switch(0, FaultSwitchMode::Down);
        assert_reset(talked);
        assert_reset(quiet);
        assert_reset(fwd.connect(0));
        other.write_all(b"ping").unwrap();
        other.read_exact(&mut [0; 4]).unwrap();
        let down = fwd.counters_until(|c| c.targets[0].connections == 3 && c.targets[0].open == 0);
        assert_eq!(down.targets[1].open, 1);

        std::fs::remove_file(&fwd.fault).unwrap();
        until("a new connection to db passes", || {
            fwd.request(0, b"ping") == b"ping"
        });
        drop(other);
        let after = fwd.counters_until(|c| c.targets.iter().all(|target| target.open == 0));
        let drops: Vec<u64> = after.targets.iter().map(|target| target.drops).collect();
        assert_eq!(drops, [0, 0], "a reset is not a drop");
    }

    #[test]
    fn a_slow_fault_holds_the_first_byte_of_each_new_connection_while_the_file_is_there() {
        const DELAY_MS: u64 = 400;
        let delay = Duration::from_millis(DELAY_MS);
        let fwd = Forwarder::start(vec![route("db", &[echo_upstream()])]);

        fwd.switch(0, FaultSwitchMode::Slow { delay_ms: DELAY_MS });
        // The watcher reads the file every 100 ms, so the switch takes effect a moment later.
        until("the delay", || fwd.first_byte(0) >= delay);
        assert!(fwd.first_byte(0) >= delay);
        assert_eq!(fwd.request(0, b"ping"), b"ping");

        std::fs::remove_file(&fwd.fault).unwrap();
        until("the end of the delay", || fwd.first_byte(0) < delay);
    }

    /// Within the read timeout of `connect`, so a connection the relay keeps open fails it too.
    fn assert_reset(mut stream: TcpStream) {
        let read = stream.read(&mut [0]);
        assert!(
            matches!(&read, Err(error) if error.kind() == ErrorKind::ConnectionReset),
            "{read:?}"
        );
    }
}

#[cfg(unix)]
mod process {
    use std::fs;
    use std::path::Path;
    use std::process::{Child, Command as Process};
    use std::time::Instant;

    use super::*;

    #[test]
    fn run_exits_with_the_code_of_the_command_and_its_group_ends_with_it() {
        for (script, code) in [("sleep 30 & exit 3", 3), ("sleep 30 & kill -KILL $$", 137)] {
            let temp = tempfile::tempdir().unwrap();
            let status = temp.path().join("status.json");
            let exit = guest(&run(&status, script, &[])).status().unwrap();
            assert_eq!(exit.code(), Some(code), "{script}");
            let recorded = recorded(&status);
            assert_eq!(recorded["code"], code, "{script}");
            assert_eq!(recorded["pgid"], recorded["pid"]);
            assert!(recorded["exitedAt"].is_u64());
            assert_group_ends(recorded["pgid"].as_i64().unwrap() as libc::pid_t);
        }
    }

    #[test]
    fn stop_signals_the_command_and_reports_its_exit() {
        let (mut run, status, _temp) = service("trap 'exit 7' TERM");
        let report = stop(&status, LONG);
        assert_eq!((report.killed, report.code), (false, Some(7)), "{report:?}");
        assert_eq!(run.wait().unwrap().code(), Some(7));
    }

    /// The pid of a command that has ended may be another process's by now.
    #[test]
    fn stop_leaves_the_pid_of_an_ended_command_alone() {
        let temp = tempfile::tempdir().unwrap();
        let status = temp.path().join("status.json");
        let mut other = Process::new("sleep").arg("30").spawn().unwrap();
        let pid = other.id();
        let ended = serde_json::json!({"pid": pid, "pgid": pid, "code": 0, "exitedAt": 1});
        fs::write(&status, ended.to_string()).unwrap();
        let report = StopReport {
            after_ms: 0,
            killed: false,
            code: Some(0),
        };
        assert_eq!(stop(&status, LONG), report);
        assert!(other.try_wait().unwrap().is_none());
        let _ = other.kill();
        let _ = other.wait();
    }

    #[test]
    fn stop_refuses_a_status_file_that_names_no_command() {
        use std::os::unix::process::CommandExt;

        let temp = tempfile::tempdir().unwrap();
        let status = temp.path().join("status.json");
        let whole_group = serde_json::json!({"pid": 0, "pgid": 0, "code": null, "exitedAt": null});
        fs::write(&status, whole_group.to_string()).unwrap();
        let mut stop = guest(&Command::Stop(StopArgs {
            status: status.clone(),
            signal: "SIGTERM".into(),
            grace: Duration::ZERO,
        }));
        // A group of its own: a `stop` that signals pid 0 reaches itself, not the test.
        let output = stop.process_group(0).output().unwrap();
        assert_eq!(output.status.code(), Some(125));
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("pid 0 in group 0 is not a command's"),
            "{stderr}"
        );
    }

    #[test]
    fn a_command_that_ignores_the_signal_is_killed_with_its_group_at_the_grace() {
        let (mut run, status, _temp) = service("trap '' TERM");
        let report = stop(&status, Duration::from_millis(300));
        assert_eq!(
            (report.killed, report.code),
            (true, Some(137)),
            "{report:?}"
        );
        assert!((300..1300).contains(&report.after_ms), "{report:?}");
        assert_eq!(run.wait().unwrap().code(), Some(137));
        assert_group_ends(recorded(&status)["pgid"].as_i64().unwrap() as libc::pid_t);
    }

    #[test]
    fn exec_writes_the_outputs_to_files_and_its_group_ends_with_the_command() {
        let temp = tempfile::tempdir().unwrap();
        let out = temp.path().join("probe.jsonl");
        // In a guest root makes the files, in a folder the service can write.
        let victim = temp.path().join("victim");
        fs::write(&victim, "kept").unwrap();
        std::os::unix::fs::symlink(&victim, temp.path().join("probe.jsonl.err")).unwrap();
        let script = "sleep 30 & echo $$; echo oops >&2; exit 3";
        let exit = exec(&out, LONG, "", script).status().unwrap();
        assert_eq!(exit.code(), Some(3));
        let err = fs::read_to_string(temp.path().join("probe.jsonl.err")).unwrap();
        assert_eq!(err, "oops\n");
        assert_eq!(fs::read_to_string(&victim).unwrap(), "kept");
        assert_group_ends(fs::read_to_string(&out).unwrap().trim().parse().unwrap());
    }

    #[test]
    fn exec_kills_the_group_at_the_deadline() {
        let temp = tempfile::tempdir().unwrap();
        let out = temp.path().join("probe.jsonl");
        let deadline = Duration::from_millis(500);
        let started = Instant::now();
        let exit = exec(&out, deadline, "", "sleep 30 & echo $$; wait")
            .status()
            .unwrap();
        assert_eq!(exit.code(), Some(124));
        assert!((deadline..LONG).contains(&started.elapsed()));
        assert_group_ends(fs::read_to_string(&out).unwrap().trim().parse().unwrap());
    }

    /// In a guest `exec` starts as root; without root here the case is skipped.
    #[cfg(target_os = "linux")]
    #[test]
    fn exec_under_root_takes_the_probe_group_and_the_user() {
        if unsafe { libc::geteuid() } != 0 {
            return skip_root_case(
                "exec_under_root_takes_the_probe_group_and_the_user",
                "not root",
            );
        }
        let nobody = Process::new("id").args(["-u", "nobody"]).output().unwrap();
        let nobody = String::from_utf8(nobody.stdout).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let out = temp.path().join("probe.jsonl");
        let gid = guest::PROBE_GID;
        for (user, uid) in [("", "0"), ("nobody", nobody.trim()), ("4321:4321", "4321")] {
            let exit = exec(&out, LONG, user, "id -u; id -g; id -G")
                .status()
                .unwrap();
            assert_eq!(exit.code(), Some(0), "{user:?}");
            let ids = fs::read_to_string(&out).unwrap();
            assert_eq!(ids, format!("{uid}\n{gid}\n{gid}\n"), "{user:?}");
        }
    }

    /// `sh -c <script>` with `$1`, `$2`... set to `args`.
    fn sh(script: &str, args: &[&Path]) -> Vec<String> {
        let shell = ["sh", "-c", script, "sh"].map(String::from);
        let args = args.iter().map(|arg| arg.display().to_string());
        shell.into_iter().chain(args).collect()
    }

    fn run(status: &Path, script: &str, args: &[&Path]) -> Command {
        Command::Run(RunArgs {
            status: status.into(),
            command: sh(script, args),
        })
    }

    /// `run` of a shell that sets `traps` and then waits on a child. Returns once the status file
    /// is there and the traps are set, so the signal cannot come first.
    fn service(traps: &str) -> (Child, PathBuf, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let status = temp.path().join("status.json");
        let ready = temp.path().join("ready");
        let script = format!("{traps}; : > \"$1\"; sleep 30 & wait");
        let child = guest(&run(&status, &script, &[&ready])).spawn().unwrap();
        wait_for(&status);
        wait_for(&ready);
        (child, status, temp)
    }

    fn stop(status: &Path, grace: Duration) -> StopReport {
        let stop = Command::Stop(StopArgs {
            status: status.into(),
            signal: "SIGTERM".into(),
            grace,
        });
        let output = guest(&stop).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        StopReport::parse(&String::from_utf8(output.stdout).unwrap()).unwrap()
    }

    fn exec(out: &Path, deadline: Duration, user: &str, script: &str) -> Process {
        guest(&Command::Exec(ExecArgs {
            out: out.into(),
            deadline,
            user: user.into(),
            argv: sh(script, &[]),
        }))
    }

    fn recorded(status: &Path) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(status).unwrap()).unwrap()
    }

    fn wait_for(path: &Path) {
        until(&path.display().to_string(), || path.exists());
    }

    /// Init reaps the killed children, so they may stay in the group a moment as zombies.
    fn assert_group_ends(pgid: libc::pid_t) {
        until(&format!("the end of group {pgid}"), || unsafe {
            libc::killpg(pgid, 0) != 0
        });
    }
}

/// The error cases run as any user. Trimming needs root and an ext4 disk, which the test makes on a
/// loop device, as an owned disk of a guest is; without them it is skipped.
#[cfg(target_os = "linux")]
mod trim {
    use std::fs::{self, File};
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;
    use std::process::{Command as Process, Output};

    use super::*;

    const JUNK: usize = 16 << 20;

    #[test]
    fn trim_tries_every_mount_and_names_each_one_it_could_not_trim() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing");
        let output = trim(&[Path::new("/proc"), &missing]);
        assert_eq!(output.status.code(), Some(125));
        let stderr = String::from_utf8(output.stderr).unwrap();
        let lines: Vec<&str> = stderr.lines().collect();
        assert_eq!(lines.len(), 2, "{stderr}");
        assert_eq!(
            lines[0],
            "stand-guest trim: FITRIM /proc: not supported by its file system"
        );
        let open = format!("stand-guest trim: open {}: ", missing.display());
        assert!(lines[1].starts_with(&open), "{stderr}");
    }

    #[test]
    fn trim_under_root_frees_the_blocks_of_deleted_files() {
        const NAME: &str = "trim_under_root_frees_the_blocks_of_deleted_files";
        if unsafe { libc::geteuid() } != 0 {
            return skip_root_case(NAME, "not root");
        }
        let disk = match Ext4::mount() {
            Ok(disk) => disk,
            Err(why) => return skip_root_case(NAME, &why),
        };
        let junk = disk.dir.join("junk");
        fs::write(&junk, vec![0xA5; JUNK]).unwrap();
        File::open(&junk).unwrap().sync_all().unwrap();
        fs::remove_file(&junk).unwrap();
        // Commits the delete, so the blocks are free when FITRIM looks.
        unsafe { libc::sync() };
        let before = disk.allocated();

        let output = trim(&[&disk.dir]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let freed = before - disk.allocated();
        assert!(
            freed >= JUNK as u64,
            "the image gave back {freed} bytes, less than the {JUNK} deleted"
        );
    }

    fn trim(mounts: &[&Path]) -> Output {
        let mounts = mounts.iter().map(|mount| mount.to_path_buf()).collect();
        guest(&Command::Trim(TrimArgs { mounts })).output().unwrap()
    }

    /// Mounted from a sparse image, so a discard on it gives the image's blocks back.
    struct Ext4 {
        image: PathBuf,
        dir: PathBuf,
        _temp: tempfile::TempDir,
    }

    impl Ext4 {
        fn mount() -> Result<Ext4, String> {
            let temp = tempfile::tempdir().unwrap();
            let image = temp.path().join("disk.img");
            let dir = temp.path().join("w");
            File::create(&image).unwrap().set_len(64 << 20).unwrap();
            fs::create_dir(&dir).unwrap();
            // The inode tables and the journal are written now, not by a kernel thread while the
            // image is measured.
            let lazy = "lazy_itable_init=0,lazy_journal_init=0";
            ran(Process::new("mkfs.ext4")
                .args(["-q", "-F", "-b", "4096", "-E", lazy])
                .arg(&image))?;
            ran(Process::new("mount")
                .args(["-o", "loop"])
                .arg(&image)
                .arg(&dir))?;
            Ok(Ext4 {
                image,
                dir,
                _temp: temp,
            })
        }

        fn allocated(&self) -> u64 {
            fs::metadata(&self.image).unwrap().blocks() * 512
        }
    }

    /// `mount -o loop` frees the loop device on the unmount.
    impl Drop for Ext4 {
        fn drop(&mut self) {
            let _ = Process::new("umount").arg(&self.dir).status();
        }
    }

    fn ran(command: &mut Process) -> Result<(), String> {
        let shown = format!("{command:?}");
        match command.output() {
            Ok(output) if output.status.success() => Ok(()),
            Ok(output) => Err(format!(
                "{shown}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(error) => Err(format!("{shown}: {error}")),
        }
    }
}

/// A root case skips where it cannot run, unless `STAND_GUEST_ROOT_REQUIRED` is set: CI sets it when
/// it runs these cases again under `sudo`, so there a case that cannot run fails.
#[cfg(target_os = "linux")]
fn skip_root_case(name: &str, why: &str) {
    use std::io::Write;

    let line = format!("{name}: {why}");
    assert!(
        std::env::var_os("STAND_GUEST_ROOT_REQUIRED").is_none(),
        "{line}"
    );
    // Past the harness's capture, so the skip shows in a passing run.
    let _ = writeln!(std::io::stderr(), "skipped {line}");
}
