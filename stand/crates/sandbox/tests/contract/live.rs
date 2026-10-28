//! The shared cases on `Microsandbox`, and the cases only a real guest shows. Run with
//! `STOP_SLOP_STAND_LIVE=1` (and `STOP_SLOP_STAND_GUEST=<musl build>` before the pin)
//! `cargo test --locked -p sandbox --features microsandbox --test contract -- --test-threads=1`:
//! one case at a time, since `leftovers` sees the whole namespace. On Windows build with
//! `--no-run` and start the binary it names: cargo keeps its children in a job without breakaway,
//! where a VM from a snapshot, a detached process, cannot start.

use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use ::microsandbox as msb;
use camino::{Utf8Path, Utf8PathBuf};
use sandbox::{
    ExecOutcome, Exposed, Fault, FaultMode, ImageRef, Launch, Metrics, Microsandbox, Outbound,
    Peer, PortStats, Sandbox, SandboxError, SandboxId, Seed, SnapshotId, VolumeName, Waitable,
};
use tempfile::TempDir;

use super::images::{ALPINE, BUSYBOX, NGINX, NODE, POSTGRES};
use super::{EXIT, argv, folder, launch, out, sh, volume, wait_exit};

/// `OK <status>` when the registry answers, `ERR <reason>` when the network refuses.
const FETCH: &str = "node -e \"fetch('https://registry.npmjs.org/-/ping', \
                     {signal: AbortSignal.timeout(8000)}).then(r => console.log('OK', r.status), \
                     e => console.log('ERR', String(e.cause || e)))\"";

fn live() -> Option<&'static Microsandbox> {
    if std::env::var("STOP_SLOP_STAND_LIVE").as_deref() != Ok("1") {
        return None;
    }
    static LIVE: OnceLock<Microsandbox> = OnceLock::new();
    Some(LIVE.get_or_init(|| {
        let _ = std::fs::remove_file(home().join("notes.txt"));
        let sandbox = Microsandbox::new(&home()).expect("the adapter");
        let pulled = sandbox.pull(&images(&[NODE, POSTGRES, NGINX]));
        pulled.expect("pull the images of the live set");
        sweep(&sandbox).expect("sweep what an earlier run left");
        sandbox
    }))
}

/// One home for every live run, kept between runs, so the runtime and the images download
/// once. On Linux and macOS it is under `/tmp`: the SDK keeps Unix sockets under its home, and
/// the per-user temp folder of macOS is too deep for `sun_path`.
fn home() -> Utf8PathBuf {
    if cfg!(unix) {
        Utf8PathBuf::from("/tmp/stop-slop-stand-live")
    } else {
        let temp = Utf8PathBuf::from_path_buf(std::env::temp_dir()).unwrap();
        temp.join("stop-slop-stand-live")
    }
}

/// What a crashed run or a panicked case left.
fn sweep(sandbox: &Microsandbox) -> Result<(), SandboxError> {
    let left = sandbox.leftovers()?;
    for id in &left.sandboxes {
        sandbox.destroy(id)?;
    }
    for snapshot in &left.snapshots {
        sandbox.drop_snapshot(snapshot)?;
    }
    Ok(())
}

/// One live case: a host folder for `/stand/out`, a sweep when it panics, and at its end the
/// check that it left nothing.
struct Case {
    sandbox: &'static Microsandbox,
    out: TempDir,
}

impl Case {
    fn start() -> Option<Case> {
        let sandbox = live()?;
        let out = tempfile::tempdir().unwrap();
        Some(Case { sandbox, out })
    }

    fn out(&self) -> &Utf8Path {
        Utf8Path::from_path(self.out.path()).unwrap()
    }

    fn write(&self, name: &str, text: &str) {
        fs::write(self.out().join(name), text).unwrap();
    }

    fn read(&self, name: &str) -> String {
        super::read(&self.out().join(name))
    }

    /// As a healthcheck runs: the image's user, the env of the service, its network. The
    /// VM needs `out` on `/stand/out`.
    fn plain(&self, id: &SandboxId, script: &str) -> (Option<i32>, String) {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!("plain-{}.out", NEXT.fetch_add(1, Ordering::Relaxed));
        let script = format!("({script}) > /stand/out/{name} 2>&1");
        let exec = self
            .sandbox
            .exec_detached(id, &argv(&["sh", "-c", &script]), EXIT);
        let what = [Waitable::Exec(exec.unwrap())];
        let exited = self.sandbox.wait(&what, Instant::now() + EXIT).unwrap();
        let code = exited.expect("the exec ends").code;
        (
            code,
            fs::read_to_string(self.out().join(name)).unwrap_or_default(),
        )
    }

    /// As a probe runs: the probe group, the image's user, the env of the service. Gives what
    /// it wrote on stdout and then on stderr.
    fn probe(&self, id: &SandboxId, script: &str) -> (ExecOutcome, String) {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!("probe-{}.out", NEXT.fetch_add(1, Ordering::Relaxed));
        let stdout = self.out().join(&name);
        let deadline = Instant::now() + EXIT;
        let outcome = self
            .sandbox
            .exec(id, &argv(&["sh", "-c", script]), deadline, &stdout);
        let said = [stdout.to_string(), format!("{stdout}.err")]
            .map(|path| fs::read_to_string(path).unwrap_or_default());
        (outcome.unwrap(), said.concat())
    }

    fn until_passes(&self, id: &SandboxId, script: &str) {
        let deadline = Instant::now() + EXIT;
        while self.plain(id, script).0 != Some(0) {
            assert!(Instant::now() < deadline, "`{script}` never passed in {id}");
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Once every connection `net` counted has closed. `net` rewrites its file at most every
    /// 100 ms, so a read right after the client's exit may still miss its last connection.
    fn settled(&self, id: &SandboxId) -> Metrics {
        std::thread::sleep(Duration::from_millis(500));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let metrics = self.sandbox.metrics(id).unwrap();
            if metrics.ports.iter().all(|port| port.open == 0) {
                return metrics;
            }
            assert!(Instant::now() < deadline, "still open: {:?}", metrics.ports);
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn finish(self, created: &[SandboxId]) {
        for id in created {
            self.sandbox.destroy(id).unwrap();
        }
        let left = self.sandbox.leftovers().unwrap();
        assert!(
            left.sandboxes.is_empty() && left.snapshots.is_empty(),
            "left behind: {left:?}"
        );
    }

    fn create(&self, batch: Vec<Launch>) -> Vec<SandboxId> {
        self.sandbox.create(&batch).expect("create")
    }

    fn log(&self, id: &SandboxId) -> String {
        String::from_utf8_lossy(&self.sandbox.logs(id).unwrap()).into_owned()
    }

    fn until_logged(&self, id: &SandboxId, line: &str) {
        let deadline = Instant::now() + EXIT;
        while !self.log(id).lines().any(|logged| logged == line) {
            assert!(Instant::now() < deadline, "no `{line}` in the log of {id}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Case {
    /// A second panic here would abort the process before the first one is reported.
    fn drop(&mut self) {
        if std::thread::panicking()
            && let Err(error) = sweep(self.sandbox)
        {
            eprintln!("the sweep after the panic failed: {error}");
        }
    }
}

fn images(names: &[&str]) -> Vec<ImageRef> {
    names.iter().map(|name| ImageRef::new(*name)).collect()
}

/// Lines for the PR description, beside the live home.
fn note(line: &str) {
    let path = home().join("notes.txt");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    writeln!(file, "{os} {arch}: {line}").unwrap();
}

macro_rules! shared {
    ($($case:ident),* $(,)?) => {$(
        #[test]
        fn $case() {
            let Some(case) = Case::start() else { return };
            super::$case(case.sandbox);
            case.finish(&[]);
        }
    )*};
}

shared!(
    create_batch_and_wait,
    logs_and_metrics_until_destroy,
    stop_then_the_rest_work,
    snapshot_seeds_a_volume,
    restore_takes_its_own_launch,
    restore_refuses_a_missing_bind_and_a_new_volume,
    leftovers_sees_what_was_made,
    pull_of_an_unknown_image_is_not_found,
    pull_gives_digests_in_order,
    a_snapshot_outlives_its_vm,
    tree_lands_before_the_command,
    copy_out_brings_the_scratch_folder,
    create_returns_once_the_command_runs,
    exec_writes_stdout_and_times_out,
    a_hung_healthcheck_is_killed_at_its_timeout,
    a_missing_command_exits_127_and_stops_at_once,
);

/// Row 88, a stop rule of T-11: without a digest the setup cache key cannot see a new image
/// under the same tag.
#[test]
fn live_pull_gives_a_digest_per_image_and_the_guest_version() {
    let Some(case) = Case::start() else { return };
    let pulled = case.sandbox.pull(&images(&[NODE, POSTGRES])).unwrap();
    let names: Vec<&str> = pulled
        .digests
        .iter()
        .map(|(image, _)| image.as_str())
        .collect();
    assert_eq!(names, [NODE, POSTGRES]);
    let digests = pulled.digests.iter().map(|(_, digest)| digest);
    assert!(
        digests.clone().all(|digest| digest.starts_with("sha256:")),
        "{pulled:?}"
    );
    let dev = std::env::var_os("STOP_SLOP_STAND_GUEST").is_some();
    assert_eq!(pulled.guest.starts_with("dev-"), dev, "{pulled:?}");
    note(&format!(
        "pull: {:?}, guest {}",
        pulled.digests, pulled.guest
    ));
    case.finish(&[]);
}

/// Row 72, a stop rule of T-11: the signal of `stand-guest stop` must reach the service, also a
/// `SIGQUIT` (the stop signal of nginx images), and what the trap says last is in `logs` once
/// `stop` returns. An ignored signal survives `exec`, so no command of the service may start
/// with one. Each service logs `ready` once its trap is set, the clean one on stderr and after
/// the signals ignored or blocked in `run`, as the agent starts it, and in a command of the
/// service; not in the shell itself, since busybox `sh` ignores `SIGQUIT` on its own and blocks
/// every signal while it waits for a child.
#[test]
fn live_stop_signals_the_service_and_kills_it_at_grace() {
    let Some(case) = Case::start() else { return };
    let mut clean = launch("clean", ALPINE);
    clean.command = sh(
        "awk '/^Sig(Ign|Blk)/ {print \"run\", $0}' /proc/$PPID/status; \
         awk '/^Sig(Ign|Blk)/ {print \"service\", $0}' /proc/self/status; \
         trap 'echo bye; exit 0' TERM; echo ready >&2; while true; do sleep 0.1; done",
    );
    let mut quitter = launch("quitter", ALPINE);
    quitter.command =
        sh("trap 'echo bye; exit 0' QUIT; echo ready; while true; do sleep 0.1; done");
    quitter.stop_signal = Some("SIGQUIT".into());
    let mut stubborn = launch("stubborn", ALPINE);
    stubborn.command = sh("trap '' TERM; echo ready; while true; do sleep 0.1; done");
    stubborn.stop_grace = Duration::from_secs(2);
    let ids = case.create(vec![clean, quitter, stubborn]);
    for id in &ids {
        case.until_logged(id, "ready");
    }
    let log = case.log(&ids[0]);
    let signals: Vec<&str> = log.lines().filter(|line| line.contains(" Sig")).collect();
    note(&format!("signals ignored and blocked: {signals:?}"));
    let ignored = "service SigIgn:\t0000000000000000";
    assert!(signals.contains(&ignored), "{signals:?}");

    for id in &ids[..2] {
        let stopped = case.sandbox.stop(id).unwrap();
        let fast = stopped.after < Duration::from_secs(2);
        assert!(!stopped.killed && fast, "{id}: {stopped:?}");
        let last = case.log(id);
        assert!(last.lines().any(|line| line == "bye"), "{id}: {last}");
        assert_eq!(wait_exit(case.sandbox, id), Some(0), "{id}: the trap ran");
    }
    let killed = case.sandbox.stop(&ids[2]).unwrap();
    let at_grace = Duration::from_millis(1900)..Duration::from_secs(4);
    assert!(
        killed.killed && at_grace.contains(&killed.after),
        "{killed:?}"
    );
    assert_eq!(wait_exit(case.sandbox, &ids[2]), Some(128 + 9));
    case.finish(&ids);
}

/// The agent keeps only the last MiB of a job's output; `logs` has the service's from its
/// first line, both streams.
#[test]
fn live_logs_keep_the_whole_output_of_a_service() {
    let Some(case) = Case::start() else { return };
    let mut chatty = launch("chatty", ALPINE);
    chatty.command = sh("echo first; sleep 0.2; echo err >&2; sleep 0.2; \
         head -c 3145728 /dev/zero | tr '\\0' x; echo; echo last");
    let id = case.create(vec![chatty]).remove(0);
    assert_eq!(wait_exit(case.sandbox, &id), Some(0));
    let log = case.log(&id);
    let x = "x".repeat(3 << 20);
    let start = &log[..log.len().min(40)];
    assert!(
        log == format!("first\nerr\n{x}\nlast\n"),
        "{} bytes from {start:?}",
        log.len()
    );
    case.finish(&[id]);
}

fn peer(service: &str, replicas: u32, ports: &[u16]) -> Peer {
    Peer {
        service: service.into(),
        replicas,
        ports: ports.to_vec(),
    }
}

fn target<'m>(metrics: &'m Metrics, target: &str) -> &'m PortStats {
    let found = metrics.ports.iter().find(|port| port.target == target);
    found.unwrap_or_else(|| panic!("no {target} in {:?}", metrics.ports))
}

/// Prints `ready` once both listen: HTTP on 8080 (the status from the path, the hostname as
/// the body), and on 9090 a line that waits for the client to close first, as the connections
/// that fill `max_inbound` do (spike net).
const SERVER: &str = r#"
const http = require('http'), net = require('net'), os = require('os');
let listening = 0;
const up = () => { if (++listening === 2) console.log('ready'); };
http.createServer((q, s) => { s.statusCode = Number(q.url.slice(1)) || 200; s.end(os.hostname()); })
  .listen(8080, '0.0.0.0', up);
net.createServer(c => { c.on('error', () => {}); c.write('raw\n'); c.on('end', () => c.end()); })
  .listen(9090, '0.0.0.0', up);
"#;

fn server(replica: u32, peers: &[Peer], out_dir: &Utf8Path) -> Launch {
    let mut srv = launch("srv", NODE);
    srv.replica = replica;
    srv.command = Some(argv(&["node", "/stand/out/srv.js"]));
    srv.ports = vec![Exposed(8080), Exposed(9090)];
    srv.peers = peers.to_vec();
    srv.mounts = vec![out(out_dir)];
    srv
}

/// One line each: `<status> <body>` of two HTTP requests, the line of 9090, how a connection
/// to the name without `expose` went.
const CLIENT: &str = r#"
const http = require('http'), net = require('net'), fs = require('fs');
const get = p => new Promise(ok => http.get({ host: 'srv', port: 8080, path: p, agent: false }, r => {
  let b = ''; r.on('data', d => b += d); r.on('end', () => ok(`${r.statusCode} ${b}`));
}).on('error', e => ok(String(e))));
const raw = () => new Promise(ok => { const c = net.connect(9090, 'srv'); let b = '';
  c.on('data', d => { b += d; c.end(); }); c.on('close', () => ok(b.trim())); c.on('error', e => ok(String(e))); });
const ghost = () => new Promise(ok => { const c = net.connect(1234, 'ghost');
  c.on('connect', () => { c.destroy(); ok('connected'); }); c.on('error', e => ok(e.code)); });
(async () => {
  const lines = [await get('/200'), await get('/503'), await raw(), await ghost()];
  fs.writeFileSync('/stand/out/result.txt', lines.join('\n') + '\n');
})();
"#;

#[test]
fn live_names_reach_peers_in_turn_and_the_relay_counts_them() {
    let Some(case) = Case::start() else { return };
    case.write("srv.js", SERVER);
    case.write("client.js", CLIENT);
    let peers = [
        peer("srv", 2, &[8080, 9090]),
        peer("ghost", 1, &[]),
        peer("client", 1, &[]),
    ];
    let servers = (1..=2).map(|replica| server(replica, &peers, case.out()));
    let mut ids = case.create(servers.collect());
    for id in &ids {
        case.until_logged(id, "ready");
    }
    let mut client = launch("client", NODE);
    client.command = Some(argv(&["node", "/stand/out/client.js"]));
    client.peers = peers.to_vec();
    client.mounts = vec![out(case.out())];
    let client = case.create(vec![client]).remove(0);
    ids.push(client.clone());
    assert_eq!(wait_exit(case.sandbox, &client), Some(0));

    let result = case.read("result.txt");
    let lines: Vec<&str> = result.lines().collect();
    let [ok, failed, raw, _ghost] = lines.as_slice() else {
        panic!("four lines: {result}");
    };
    let (ok, failed) = (ok.split_once(' '), failed.split_once(' '));
    let (Some(("200", first)), Some(("503", second))) = (ok, failed) else {
        panic!("{result}");
    };
    assert_ne!(
        first, second,
        "two connections go to the two replicas in turn"
    );
    assert_eq!(*raw, "raw");

    let metrics = case.settled(&client);
    let http = target(&metrics, "srv:8080");
    assert_eq!((http.connections, http.http), (2, Some([0, 1, 0, 0, 1])));
    assert!(http.bytes_out > 0 && http.bytes_in > 0, "{http:?}");
    let line = target(&metrics, "srv:9090");
    assert_eq!((line.connections, line.bytes_in, line.http), (1, 4, None));
    let ghost = metrics.unexposed.iter().find(|name| name.name == "ghost");
    assert_eq!(ghost.map(|ghost| ghost.attempts), Some(1), "{metrics:?}");
    case.finish(&ids);
}

/// Also notes whether the guest has IPv6 beside `lo`, which microsandbox gives it only when the
/// host has an IPv6 route.
#[test]
fn live_off_refuses_an_outside_address_at_once_and_public_reaches_it() {
    let Some(case) = Case::start() else { return };
    let mut off = launch("off", NODE);
    off.command = sh("sleep 600");
    off.mounts = vec![out(case.out())];
    let mut public = off.clone();
    public.service = "public".into();
    public.outbound = Outbound::Public;
    let ids = case.create(vec![off, public]);
    let started = Instant::now();
    let (_, refused) = case.plain(&ids[0], FETCH);
    let fast = started.elapsed() < Duration::from_secs(5);
    assert!(refused.starts_with("ERR") && fast, "{refused}");
    let (_, answered) = case.plain(&ids[1], FETCH);
    assert!(answered.starts_with("OK 200"), "{answered}");

    let (_, ipv6) = case.plain(&ids[0], "cat /proc/net/if_inet6 /proc/net/ipv6_route");
    let off_lo: Vec<&str> = ipv6.lines().filter(|line| !line.ends_with(" lo")).collect();
    note(&format!(
        "ipv6 beside lo (addresses and routes): {off_lo:?}"
    ));
    case.finish(&ids);
}

fn postgres(service: &str, peers: &[Peer], out_dir: &Utf8Path) -> Launch {
    let mut db = launch(service, POSTGRES);
    db.env.insert("POSTGRES_PASSWORD".into(), "pw".into());
    db.command = Some(argv(&[
        "postgres",
        "-c",
        &format!("cluster_name={service}"),
    ]));
    db.ports = vec![Exposed(5432)];
    db.peers = peers.to_vec();
    db.mounts = vec![out(out_dir)];
    db
}

/// A stop rule of T-11 (row 80), the first case on the Mac: each Postgres listens on
/// `0.0.0.0:5432`, and `db2:5432` from `db` must still reach `db2`, not `db`'s own Postgres.
#[test]
fn live_redirect_stands_beside_the_wildcard_listener_of_a_service() {
    let Some(case) = Case::start() else { return };
    let peers = [peer("db", 1, &[5432]), peer("db2", 1, &[5432])];
    let dbs = ["db", "db2"].map(|service| postgres(service, &peers, case.out()));
    let ids = case.create(dbs.to_vec());
    for id in &ids {
        case.until_passes(id, PG_READY);
    }
    let ask = |host: &str| {
        let sql = format!("PGPASSWORD=pw psql -h {host} -U postgres -tAc 'show cluster_name'");
        let (code, out) = case.plain(&ids[0], &sql);
        (code, out.trim().to_owned())
    };
    assert_eq!(ask("127.0.0.1"), (Some(0), "db".to_owned()));
    assert_eq!(ask("db2"), (Some(0), "db2".to_owned()), "db2:5432 from db");
    case.finish(&ids);
}

/// busybox `nc`: 0 once it connected, and it closes at once; on stderr why not (a refusal only
/// with `-vv`).
fn dial(host: &str, port: u16) -> String {
    format!("nc -zvv -w 5 {host} {port}")
}

/// Row 62, a stop rule of T-11: a probe runs in the probe group as the image's user (root in
/// `postgres`, 101 in `nginx-unprivileged`) and reaches only its own guest's loopback, over
/// IPv4 and IPv6. A peer by name, an outside address and an IPv6 address beside `::1` refuse
/// it at once, while the same dials outside the probe group connect. `fd00::1` on `lo` stands
/// for an outside IPv6 address, which a host without an IPv6 route does not give the guest.
#[test]
fn live_a_probe_reaches_only_loopback_with_the_probe_group_and_the_image_user() {
    let Some(case) = Case::start() else { return };
    let peers = [peer("db", 1, &[5432]), peer("web", 1, &[8080])];
    let mut db = postgres("db", &peers, case.out());
    db.outbound = Outbound::Public;
    let mut web = launch("web", NGINX);
    web.ports = vec![Exposed(8080)];
    web.peers = peers.to_vec();
    web.mounts = vec![out(case.out())];
    let ids = case.create(vec![db, web]);
    let (db, web) = (&ids[0], &ids[1]);
    let page = "wget -q -O /dev/null http://127.0.0.1:8080/";
    case.until_passes(db, PG_READY);
    case.until_passes(web, page);
    let (_, global) = case.plain(db, "ip -6 address show scope global");
    note(&format!("global ipv6 of the guest: {:?}", global.trim()));
    let added = case.plain(db, "ip -6 address add fd00::1/128 dev lo");
    assert_eq!(added.0, Some(0), "{}", added.1);

    let sql = "PGPASSWORD=pw psql -h 127.0.0.1 -U postgres -tAc 'select 1'";
    let read = case.probe(db, sql);
    assert_eq!(read, (ExecOutcome::Exited(0), "1\n".to_owned()));
    for (id, local) in [(db, dial("::1", 5432)), (web, page.to_owned())] {
        let (outcome, said) = case.probe(id, &local);
        assert_eq!(outcome, ExecOutcome::Exited(0), "`{local}` in {id}: {said}");
    }
    let away = [
        (db, "web", 8080),
        (db, "1.1.1.1", 80),
        (db, "fd00::1", 5432),
        (web, "db", 5432),
    ];
    for (id, host, port) in away {
        let dial = dial(host, port);
        let (code, said) = case.plain(id, &dial);
        assert_eq!(
            code,
            Some(0),
            "`{dial}` in {id} outside the probe group: {said}"
        );
        let (outcome, said) = case.probe(id, &dial);
        let refused = said.to_lowercase().contains("refused");
        assert!(
            outcome != ExecOutcome::Exited(0) && refused,
            "`{dial}` in {id} as a probe: {outcome:?} {said}"
        );
    }
    let (_, who) = case.probe(web, "id -u; id -g; id -G");
    let who: Vec<&str> = who.split_whitespace().collect();
    assert_eq!(who, ["101", "65533", "65533"], "uid, gid, groups");
    case.finish(&ids);
}

const BURST: &str = r#"
const net = require('net');
let started = 0;
const one = () => new Promise(ok => { const c = net.connect(9090, 'srv');
  c.on('data', () => c.end()); c.on('close', ok); c.on('error', ok); });
const lane = async () => { while (started < 300) { started++; await one(); } };
Promise.all(Array.from({ length: 32 }, lane));
"#;

/// Row 39: 300 new connections, 32 at a time, to one sandbox pass its `max_inbound` of 256
/// (spike net: 44 refusals within a second). One at a time the count is not stable, so the
/// check is `>= 1`.
#[test]
fn live_a_burst_of_connections_hits_the_limit_of_256() {
    let Some(case) = Case::start() else { return };
    case.write("srv.js", SERVER);
    case.write("burst.js", BURST);
    let peers = [peer("srv", 1, &[8080, 9090]), peer("burst", 1, &[])];
    let mut ids = case.create(vec![server(1, &peers, case.out())]);
    case.until_logged(&ids[0], "ready");
    let mut burst = launch("burst", NODE);
    burst.command = Some(argv(&["node", "/stand/out/burst.js"]));
    burst.peers = peers.to_vec();
    burst.mounts = vec![out(case.out())];
    let burst = case.create(vec![burst]).remove(0);
    ids.push(burst.clone());
    assert_eq!(wait_exit(case.sandbox, &burst), Some(0));
    let metrics = case.settled(&burst);
    let line = target(&metrics, "srv:9090");
    note(&format!("burst of 300, 32 at a time: {line:?}"));
    assert!(line.connections == 300 && line.drops >= 1, "{line:?}");
    case.finish(&ids);
}

/// Holds a connection to `srv:9090` open; once `/tmp/go` shows (the guest's own file, no
/// virtio-fs in the way), dials a new one and logs `next first <ms>` at its first byte, else
/// how it ended.
const HOLDER: &str = r#"
const net = require('net'), fs = require('fs');
const held = net.connect(9090, 'srv');
held.once('data', () => console.log('held ready'));
held.on('error', e => console.log('held ' + e.code));
const dial = () => new Promise(ok => {
  const c = net.connect(9090, 'srv'), started = Date.now();
  c.on('data', () => { ok('first ' + (Date.now() - started)); c.destroy(); });
  c.on('error', e => ok(e.code)); c.on('close', () => ok('closed'));
});
(async () => {
  while (!fs.existsSync('/tmp/go')) await new Promise(r => setTimeout(r, 5));
  console.log('next ' + await dial());
  held.destroy();
})();
"#;

/// The T-09 stage 4 question: `fault` writes the switch inside the guest and returns after
/// `net` has read it, so a connection the guest opens right after the return sees the fault:
/// `down` resets the open connection and the new one, `slow` holds the new one's first byte.
#[test]
fn live_down_and_slow_reach_the_relay_before_fault_returns() {
    let Some(case) = Case::start() else { return };
    case.write("srv.js", SERVER);
    case.write("holder.js", HOLDER);
    let peers = [peer("srv", 1, &[8080, 9090]), peer("holder", 1, &[])];
    let ids = case.create(vec![server(1, &peers, case.out())]);
    case.until_logged(&ids[0], "ready");
    let delay = Duration::from_millis(600);
    for mode in [FaultMode::Down, FaultMode::Slow(delay)] {
        let mut holder = launch("holder", NODE);
        holder.command = Some(argv(&["node", "/stand/out/holder.js"]));
        holder.peers = peers.to_vec();
        holder.mounts = vec![out(case.out())];
        let holder = case.create(vec![holder]).remove(0);
        case.until_logged(&holder, "held ready");
        let fault = Fault {
            target: "srv:9090".into(),
            mode,
        };
        let started = Instant::now();
        let faulted = std::slice::from_ref(&holder);
        case.sandbox.fault(faulted, &fault).unwrap();
        let took = started.elapsed();
        assert_eq!(
            case.probe(&holder, "touch /tmp/go").0,
            ExecOutcome::Exited(0)
        );
        assert_eq!(wait_exit(case.sandbox, &holder), Some(0));
        let log = case.log(&holder);
        let lines: Vec<&str> = log.lines().collect();
        note(&format!("{mode:?}: fault() took {took:?}, then {lines:?}"));
        match mode {
            FaultMode::Down => {
                let reset = ["held ECONNRESET", "next ECONNRESET"];
                assert!(reset.iter().all(|line| lines.contains(line)), "{lines:?}");
            }
            FaultMode::Slow(delay) => {
                let first = lines
                    .iter()
                    .find_map(|line| line.strip_prefix("next first "));
                let first: u128 = first.and_then(|ms| ms.parse().ok()).unwrap_or(0);
                assert!(first >= delay.as_millis(), "{lines:?}");
            }
        }
        case.sandbox.destroy(&holder).unwrap();
    }
    case.finish(&ids);
}

/// Orchestrator decision 8 of T-11: the service's user (101 in nginx-unprivileged) writes the
/// status file of `run` in `/stand/run`, but nothing in `/stand/net`, where root keeps the
/// counters and the fault switch; it reads the names.
#[test]
fn live_the_image_user_writes_run_but_not_net() {
    let Some(case) = Case::start() else { return };
    let mut web = launch("web", NGINX);
    web.peers = vec![peer("web", 1, &[])];
    web.command = sh("id -u; \
         touch /stand/net/forged 2>/dev/null && echo 'net: created' || echo 'net: refused'; \
         (echo forged > /stand/net/counters.json) 2>/dev/null \
           && echo 'counters: written' || echo 'counters: refused'; \
         touch /stand/run/mine && echo 'run: created' || echo 'run: refused'; \
         grep -q ' web$' /etc/hosts && echo 'hosts: read' || echo 'hosts: unread'; \
         stat -c 'net %u:%g %a' /stand/net");
    let id = case.create(vec![web]).remove(0);
    assert_eq!(wait_exit(case.sandbox, &id), Some(0));
    let log = case.log(&id);
    let lines: Vec<&str> = log.lines().collect();
    note(&format!("the image user and the binds: {lines:?}"));
    let expected = [
        "101",
        "net: refused",
        "counters: refused",
        "run: created",
        "hosts: read",
    ];
    for line in expected {
        assert!(lines.contains(&line), "no `{line}`: {lines:?}");
    }
    case.sandbox
        .metrics(&id)
        .expect("the counters are still net's");
    case.finish(&[id]);
}

/// For the SDK's own calls in the checks: they reach the adapter's home, the process-wide
/// default since the first `pull`. One runtime for every call, since the SDK may keep what it
/// opened on it; a worker with a big stack, as in the adapter.
fn sdk<T: Send + 'static>(call: impl Future<Output = T> + Send + 'static) -> T {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    let runtime = RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_stack_size(16 << 20)
            .enable_all()
            .build()
            .unwrap()
    });
    runtime.block_on(async { tokio::spawn(call).await.unwrap() })
}

/// Decides the port: owned disks live in the SDK's folder of their VM and go with it, so the
/// port has no `drop_volumes`.
#[test]
fn live_owned_disks_die_with_their_vm() {
    let Some(case) = Case::start() else { return };
    let mut keeper = launch("keeper", ALPINE);
    keeper.command = sh("echo data > /data/file && sleep 600");
    keeper.mounts = vec![volume(&VolumeName::new("data"), "/data")];
    let id = case.create(vec![keeper]).remove(0);
    let vm = home().join("sandboxes").join(id.as_str());
    let disks: Vec<Utf8PathBuf> = fs::read_dir(vm.join("owned-volumes"))
        .unwrap()
        .map(|mount| Utf8PathBuf::try_from(mount.unwrap().path().join("disk.raw")).unwrap())
        .filter(|disk| disk.is_file())
        .collect();
    assert_eq!(disks.len(), 1, "one owned disk under {vm}: {disks:?}");
    case.sandbox.destroy(&id).unwrap();
    assert!(!vm.exists(), "{vm} outlived its VM");
    case.finish(&[]);
}

/// By its digest in the listing, as the adapter finds it.
fn listed(snapshot: &SnapshotId) -> msb::SnapshotHandle {
    let digest = snapshot.as_str().to_owned();
    sdk(async move {
        let all = msb::Snapshot::list().await.unwrap();
        let found = all.into_iter().find(|listed| listed.digest() == digest);
        found.expect("listed")
    })
}

fn cached(image: &'static str) -> bool {
    sdk(async move { msb::Image::get(image).await.is_ok() })
}

/// A snapshot pins its image, so `clean` drops snapshots before images.
#[test]
fn live_a_snapshot_carries_the_label_and_drops_before_its_image() {
    let Some(case) = Case::start() else { return };
    let kept = [ALPINE, NODE, POSTGRES, NGINX];
    let keep = images(&kept);
    case.sandbox.pull(&images(&[BUSYBOX])).unwrap();
    let mut boxed = launch("boxed", BUSYBOX);
    boxed.command = sh("sleep 600");
    let id = case.create(vec![boxed]).remove(0);
    case.sandbox.stop(&id).unwrap();
    let snapshot = case.sandbox.snapshot(&id).unwrap();
    case.sandbox.destroy(&id).unwrap();
    let left = case.sandbox.leftovers().unwrap();
    assert!(left.snapshots.contains(&snapshot), "{left:?}");
    let found = listed(&snapshot);
    let labels = sdk(async move { found.open().await.unwrap().labels().clone() });
    let label = labels.get("stop-slop-stand").map(String::as_str);
    assert_eq!(label, Some("1"), "{labels:?}");
    case.sandbox.prune_images(&keep).unwrap();
    assert!(cached(BUSYBOX), "pruned while a snapshot pins it");
    case.sandbox.drop_snapshot(&snapshot).unwrap();
    case.sandbox.prune_images(&keep).unwrap();
    assert!(!cached(BUSYBOX), "kept after its snapshot went");
    let lost: Vec<&str> = kept.into_iter().filter(|image| !cached(image)).collect();
    assert!(lost.is_empty(), "pruned though kept: {lost:?}");
    case.finish(&[]);
}

/// `RestoreBuilder` takes no label, so `leftovers` finds a VM from a snapshot by its name;
/// whether it carries the label of the namespace anyway goes to the notes.
#[test]
fn live_a_vm_from_a_snapshot_is_in_the_leftovers() {
    let Some(case) = Case::start() else { return };
    let mut source = launch("source", ALPINE);
    source.command = sh("sleep 600");
    let id = case.create(vec![source.clone()]).remove(0);
    case.sandbox.stop(&id).unwrap();
    let snapshot = case.sandbox.snapshot(&id).unwrap();
    case.sandbox.destroy(&id).unwrap();
    source.seed = Some(Seed::Snapshot(snapshot.clone()));
    let restored = case.create(vec![source]).remove(0);
    let left = case.sandbox.leftovers().unwrap();
    assert!(left.sandboxes.contains(&restored), "{left:?}");
    let labelled = sdk(async {
        let page = msb::Sandbox::list_with(|list| {
            let list = list.limit(msb::sandbox::MAX_SANDBOX_LIST_LIMIT);
            list.label("stop-slop-stand", "1")
        })
        .await;
        page.map(|page| {
            let names = page.sandboxes.iter().map(|vm| vm.name().to_owned());
            names.collect::<Vec<_>>()
        })
    });
    let carries = labelled
        .unwrap()
        .iter()
        .any(|name| name == restored.as_str());
    note(&format!(
        "a VM from a snapshot carries the namespace label: {carries}"
    ));
    case.sandbox.destroy(&restored).unwrap();
    case.sandbox.drop_snapshot(&snapshot).unwrap();
    case.finish(&[]);
}

/// The snapshot of a setup cache entry outlives the run that made it, and that run's folders.
#[test]
fn live_a_vm_from_a_snapshot_starts_after_its_source_and_folders_are_gone() {
    let Some(case) = Case::start() else { return };
    let (first, first_dir) = folder();
    let mut entry = launch("entry", ALPINE);
    entry.command = sh("echo $WHO > /stand/out/who");
    entry.env.insert("WHO".into(), "first".into());
    entry.mounts = vec![out(&first_dir)];
    let id = case.create(vec![entry.clone()]).remove(0);
    assert_eq!(wait_exit(case.sandbox, &id), Some(0));
    let snapshot = case.sandbox.snapshot(&id).unwrap();
    case.sandbox.destroy(&id).unwrap();
    drop(first);
    assert!(!first_dir.exists());
    entry.seed = Some(Seed::Snapshot(snapshot.clone()));
    entry.env.insert("WHO".into(), "second".into());
    entry.mounts = vec![out(case.out())];
    let restored = case.create(vec![entry]).remove(0);
    assert_eq!(wait_exit(case.sandbox, &restored), Some(0));
    assert_eq!(case.read("who"), "second\n");
    case.sandbox.destroy(&restored).unwrap();
    case.sandbox.drop_snapshot(&snapshot).unwrap();
    case.finish(&[]);
}

const PGDATA: &str = "/var/lib/postgresql/data";
const PG_READY: &str = "pg_isready -h 127.0.0.1 -U postgres";

/// A clean stop before the snapshot spares every start from it the recovery, 1.4-1.6 s
/// (spike state). Without a `stop_signal` the stop sends the image's `STOPSIGNAL`, `SIGINT`,
/// Postgres's fast shutdown, as Docker does; `SIGTERM` would wait for every client to leave.
#[test]
fn live_postgres_starts_from_its_snapshot_without_recovery() {
    let Some(case) = Case::start() else { return };
    let mut db = launch("db", POSTGRES);
    db.env.insert("POSTGRES_PASSWORD".into(), "pw".into());
    db.mounts = vec![volume(&VolumeName::new("pg"), PGDATA), out(case.out())];
    let first = case.create(vec![db.clone()]).remove(0);
    case.until_passes(&first, PG_READY);
    let stopped = case.sandbox.stop(&first).unwrap();
    let fast = stopped.after < db.stop_grace;
    assert!(!stopped.killed && fast, "{stopped:?}");
    let log = case.log(&first);
    assert!(log.contains("received fast shutdown request"), "{log}");
    let inspected = sdk(async {
        let detail = msb::Image::inspect(POSTGRES).await.unwrap();
        detail.config.and_then(|config| config.stop_signal)
    });
    note(&format!(
        "STOPSIGNAL of {POSTGRES} in the SDK's Image::inspect: {inspected:?}"
    ));
    let sizes = case.sandbox.volume_sizes(&first).unwrap();
    let data = match sizes.as_slice() {
        [(name, bytes)] if name.as_str() == "pg" && *bytes > 1 << 20 => *bytes,
        _ => panic!("one volume `pg` above 1 MiB: {sizes:?}"),
    };
    let snapshot = case.sandbox.snapshot(&first).unwrap();
    case.sandbox.destroy(&first).unwrap();

    db.seed = Some(Seed::Snapshot(snapshot.clone()));
    let started = Instant::now();
    let second = case.create(vec![db]).remove(0);
    let restored = started.elapsed();
    case.until_passes(&second, PG_READY);
    note(&format!(
        "postgres from its snapshot ({} MiB of data): service started {restored:?}, ready {:?}",
        data >> 20,
        started.elapsed()
    ));
    let log = case.log(&second);
    let clean = log.contains("database system was shut down at");
    assert!(clean && !log.contains("automatic recovery"), "{log}");
    case.sandbox.destroy(&second).unwrap();
    case.sandbox.drop_snapshot(&snapshot).unwrap();
    case.finish(&[]);
}

/// What the prepare service of a Node app leaves on its version disk: `install.js` stands in
/// for the package manager (44 packages of random bytes, about 450 MiB, and the one the server
/// needs) and the build.
const INSTALL: &str = r#"
const fs = require('fs'), crypto = require('crypto');
for (let p = 0; p < 44; p++) {
  const dir = `node_modules/pkg-${p}/lib`;
  fs.mkdirSync(dir, { recursive: true });
  for (let f = 0; f < 100; f++) fs.writeFileSync(`${dir}/${f}.bin`, crypto.randomBytes(105 << 10));
}
fs.mkdirSync('node_modules/who', { recursive: true });
fs.writeFileSync('node_modules/who/index.js', 'module.exports = () => process.env.WHO;');
fs.mkdirSync('dist', { recursive: true });
fs.copyFileSync('src/server.js', 'dist/server.js');
"#;
const WHO_SERVER: &str = "const who = require('who');\n\
     require('http').createServer((q, s) => s.end(who())).listen(3000, '0.0.0.0');\n";

/// No folder entries, as `source::archive` writes a tree: `tar -x` makes the folders.
fn version_tree(path: &Utf8Path) {
    let mut tar = tar::Builder::new(fs::File::create(path).unwrap());
    let files = [
        ("package.json", r#"{"name": "version", "private": true}"#),
        ("scripts/install.js", INSTALL),
        ("src/server.js", WHO_SERVER),
    ];
    for (name, text) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(text.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, name, text.as_bytes()).unwrap();
    }
    tar.finish().unwrap();
}

/// What the snapshot takes on the host's disk: the allocated blocks of its files, as `du`
/// counts them, since its disk layers are sparse files as big as the disks.
fn snapshot_bytes(snapshot: &SnapshotId) -> u64 {
    allocated_under(listed(snapshot).path().unwrap())
}

fn allocated_under(path: &Path) -> u64 {
    let meta = fs::symlink_metadata(path).unwrap();
    if meta.is_dir() {
        let entries = fs::read_dir(path).unwrap();
        entries
            .map(|entry| allocated_under(&entry.unwrap().path()))
            .sum()
    } else {
        allocated(path, &meta)
    }
}

#[cfg(unix)]
fn allocated(_: &Path, meta: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.blocks() * 512
}

#[cfg(windows)]
fn allocated(path: &Path, _: &fs::Metadata) -> u64 {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCompressedFileSizeW(name: *const u16, high: *mut u32) -> u32;
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut high = 0;
    // SAFETY: `wide` is a path ending with NUL, and `high` lives through the call.
    let low = unsafe { GetCompressedFileSizeW(wide.as_ptr(), &mut high) };
    (u64::from(high) << 32) | u64::from(low)
}

/// Rows 86 and 87, a stop rule of T-11: the prepare service installs and builds on its version
/// disk, and its snapshot (about 0.6 GiB, as spike restore's) seeds a VM of another service,
/// which sees the tree with its dependencies and build and takes its own limits, network, port,
/// bind, env, workdir and command. The restore time goes into the PR description.
#[test]
fn live_the_version_snapshot_seeds_another_service() {
    let Some(case) = Case::start() else { return };
    let (_first, first) = folder();
    let tar = first.join("tree.tar");
    version_tree(&tar);
    let mut prepare = launch("prepare", NODE);
    prepare.memory = 1 << 30;
    prepare.outbound = Outbound::Public;
    prepare.env.insert("WHO".into(), "prepare".into());
    prepare.seed = Some(Seed::Tree {
        tar,
        at: "/w/app".into(),
    });
    prepare.workdir = Some("/w/app".into());
    prepare.mounts = vec![out(&first)];
    prepare.command = sh("node scripts/install.js && echo $WHO > /stand/out/who");
    let prepare = case.create(vec![prepare]).remove(0);
    assert_eq!(wait_exit(case.sandbox, &prepare), Some(0));
    let snapshot = case.sandbox.snapshot(&prepare).unwrap();
    case.sandbox.destroy(&prepare).unwrap();
    let size = snapshot_bytes(&snapshot);
    assert!(
        size > 400 << 20,
        "the version disk in the snapshot: {size} bytes"
    );

    let peers = [peer("api", 1, &[3000]), peer("client", 1, &[])];
    let mut api = launch("api", NODE);
    api.seed = Some(Seed::Snapshot(snapshot.clone()));
    api.cpus = 2;
    api.env.insert("WHO".into(), "api".into());
    api.ports = vec![Exposed(3000)];
    api.peers = peers.to_vec();
    api.mounts = vec![out(case.out())];
    api.workdir = Some("/w/app".into());
    api.command = sh("echo $WHO > /stand/out/who && exec node dist/server.js");
    let started = Instant::now();
    let api = case.create(vec![api]).remove(0);
    let restored = started.elapsed();
    case.until_passes(&api, "wget -q -O- http://127.0.0.1:3000/");
    note(&format!(
        "the version snapshot ({} MiB) in another service: service started {restored:?}, \
         ready {:?}",
        size >> 20,
        started.elapsed()
    ));
    let limits = r#"nproc; awk '/^MemTotal/ {print int($2 / 1024)}' /proc/meminfo; echo $WHO
ls node_modules | wc -l"#;
    let (_, facts) = case.plain(&api, limits);
    let facts: Vec<&str> = facts.lines().map(str::trim).collect();
    let [cpus, mib, who, packages] = facts.as_slice() else {
        panic!("`<vCPUs> <MiB> <who> <packages>`: {facts:?}");
    };
    let mib: u32 = mib.parse().unwrap();
    assert!(*cpus == "2" && mib < 600 && *who == "api", "{facts:?}");
    assert_eq!(*packages, "45", "the dependencies from the version disk");
    let (_, outside) = case.plain(&api, FETCH);
    assert!(outside.starts_with("ERR"), "`Off` of the Launch: {outside}");
    assert_eq!(case.read("who"), "api\n");
    assert_eq!(super::read(&first.join("who")), "prepare\n");

    let mut client = launch("client", NODE);
    client.peers = peers.to_vec();
    client.mounts = vec![out(case.out())];
    client.command = sh("wget -q -O- http://api:3000/ > /stand/out/api.txt");
    let client = case.create(vec![client]).remove(0);
    assert_eq!(wait_exit(case.sandbox, &client), Some(0));
    assert_eq!(case.read("api.txt"), "api", "the port of the Launch");
    for id in [&api, &client] {
        case.sandbox.destroy(id).unwrap();
    }
    case.sandbox.drop_snapshot(&snapshot).unwrap();
    case.finish(&[]);
}

/// 64 MiB of random bytes on the volume, synced to its disk, then deleted.
const DELETED: &str = "head -c 67108864 /dev/urandom > /data/junk && sync && rm /data/junk && sync";

/// The blueprint's live check of FITRIM: the blocks of deleted files stay out of the snapshot,
/// and a trim that fails still snapshots. A tmpfs over the mount point makes it fail: FITRIM
/// reaches the tmpfs, which has none, and the disk under it keeps its deleted blocks.
#[test]
fn live_trim_keeps_deleted_blocks_out_and_a_failed_trim_still_snapshots() {
    let Some(case) = Case::start() else { return };
    let data = VolumeName::new("data");
    let mut trimmed = launch("trimmed", ALPINE);
    trimmed.mounts = vec![volume(&data, "/data")];
    trimmed.command = sh(DELETED);
    let mut untrimmed = launch("untrimmed", ALPINE);
    untrimmed.mounts = vec![volume(&data, "/data"), out(case.out())];
    untrimmed.command = sh(&format!("{DELETED} && mount -t tmpfs tmpfs /data"));
    let ids = case.create(vec![trimmed, untrimmed]);
    for id in &ids {
        assert_eq!(wait_exit(case.sandbox, id), Some(0), "{}", case.log(id));
    }
    let (code, said) = case.plain(&ids[1], "/stand/bin/stand-guest trim --mount /data");
    assert!(
        code == Some(125) && said.contains("not supported by its file system"),
        "the trim of the untrimmed VM: {code:?} {said}"
    );
    // Whether the guest's disks take discards: on CI's Linux runner the volume's did not.
    let (_, disks) = case.plain(
        &ids[1],
        "grep ' /data ' /proc/mounts; grep . /sys/block/vd*/queue/discard_max_bytes",
    );
    let disks: Vec<&str> = disks.lines().collect();
    note(&format!(
        "/data and the discard limit of each disk: {disks:?}"
    ));
    let snapshots: Vec<SnapshotId> = ids
        .iter()
        .map(|id| case.sandbox.snapshot(id).unwrap())
        .collect();
    for id in &ids {
        case.sandbox.destroy(id).unwrap();
    }
    let with = snapshot_bytes(&snapshots[0]);
    let without = snapshot_bytes(&snapshots[1]);
    note(&format!(
        "a snapshot after 64 MiB deleted: {} MiB with trim, {} MiB after a failed trim",
        with >> 20,
        without >> 20
    ));
    assert!(
        with <= without,
        "with trim {with} bytes, after a failed trim {without}"
    );
    // Only Windows showed the saving: on CI's Linux runner the volume's disk had
    // `discard_max_bytes` 0, so the trim failed there too and both snapshots were 66 MiB. The
    // Mac is not measured yet.
    if cfg!(windows) {
        assert!(
            without >= with + (48 << 20),
            "with trim {with} bytes, after a failed trim {without}"
        );
    }
    for snapshot in &snapshots {
        case.sandbox.drop_snapshot(snapshot).unwrap();
    }
    case.finish(&[]);
}
