//! The whole stand on microsandbox VMs, through the `stop-slop-stand` this package builds, with
//! `STOP_SLOP_STAND_LIVE=1`; without it every case passes at once. Run one case at a time: they
//! share the stand folder, and each starts and ends with `clean`, which drops every snapshot of it.
//!
//! The pinned `stand-guest` is downloaded and checked unless `STOP_SLOP_STAND_GUEST` names a build.
//! On Windows build with `--no-run` and start the test binary it names: cargo keeps its children
//! in a job without breakaway, where a VM from a snapshot cannot start, and the binary under test
//! is a child of this one.
//!
//! The budget counts the peak of a run, setup or a repeat, and gives a stand half the logical CPUs
//! unless `stand.toml` says more (80% at most): the whole case asks 3 vCPUs, so 6 logical CPUs, or
//! 4 with `host_share = 80`, as CI sets it; the small one asks 2. The stand also keeps 20 GiB free
//! on the disk of its folder: on Windows `TMP` moves the folder to another disk, on Linux a link at
//! its path.
//!
//! The run's folder layout (`repeat-<n>-<side>/out/`, `kept.json`, `sessions/<key>/cache/`) is a
//! law of the blueprint, so the cases read it.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write as _;
use std::process::{Command, Output};
use std::sync::{Once, OnceLock};
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use sandbox::{ImageRef, Microsandbox, Sandbox, SnapshotId};
use serde_json::Value;
use testkit::TempRepo;

const BIN: &str = env!("CARGO_BIN_EXE_stop-slop-stand");
const POSTGRES: &str = "postgres:17-alpine";
/// Debian: GNU `tar` and `id` in the guest, where the contract set has busybox.
const NODE: &str = "node:22-slim";

/// Postgres with a volume and the probe `orders`; the prepare service installs `pg`, builds and
/// migrates on the version disk; a profiled `api` with a pool of connections to `db`; the workload
/// makes 50 orders over keep-alive, twice on one state with the second pass. Migrations run in
/// prepare, which waits for `db`, so `db` runs in setup and its volume goes into the cache entry.
const RECIPE: &str = r#"
services:
  db:
    image: postgres:17-alpine
    environment: { POSTGRES_PASSWORD: stand }
    volumes: ["pgdata:/var/lib/postgresql/data"]
    expose: [5432]
    # Over TCP: during its first start the image runs a server on the socket only.
    healthcheck: { test: ["CMD", "pg_isready", "-U", "postgres", "-h", "127.0.0.1"], interval: 1s, retries: 60 }
    deploy: { resources: { limits: { cpus: "1", memory: 512m } } }
  prepare:
    image: node:22-slim
    working_dir: /w/app
    volumes: ["${STAND_REPO}:/w/app"]
    command: ["sh", "-c", "npm install --no-audit --no-fund --cache /w/.npm && npm run build && node dist/migrate.js"]
    depends_on: { db: { condition: service_healthy } }
    deploy: { resources: { limits: { cpus: "1", memory: 512m } } }
    x-stand: { network: public }
  api:
    image: node:22-slim
    working_dir: /w/app
    volumes: ["${STAND_REPO}:/w/app", ".:/harness:ro"]
    command: ["node", "dist/main.js"]
    expose: [3000]
    depends_on:
      prepare: { condition: service_completed_successfully }
      db: { condition: service_healthy }
    healthcheck: { test: ["CMD", "node", "/harness/ping.js"], interval: 1s, retries: 30 }
    deploy: { resources: { limits: { cpus: "1", memory: 384m } } }
    x-stand: { profile: node }
  load:
    image: node:22-slim
    working_dir: /harness
    volumes: [".:/harness:ro"]
    command: ["node", "load.mjs", "http://api:3000"]
    depends_on: { api: { condition: service_healthy } }
    deploy: { resources: { limits: { cpus: "1", memory: 256m } } }
volumes: { pgdata: {} }
x-stand:
  workload: load
  repeat: 2
  setup_timeout: 5m
  timeout: 2m
  second_pass: true
  probes:
    orders:
      in: db
      at: after-workload
      run: ["psql", "-U", "postgres", "-At", "-c", "select row_to_json(o) from orders o order by id"]
      key: /id
      ignore: [/at]
  expect:
    cost:
      - { measure: stand.wall, change: +10%, reason: "one more lookup per order" }
"#;

const LOAD: &str = r#"// 50 orders over keep-alive (fetch keeps its connections), one record per answer.
import { appendFileSync } from "node:fs";

const base = process.argv[2];
for (let i = 0; i < 50; i++) {
  const answer = await fetch(`${base}/orders`, { method: "POST" });
  if (!answer.ok) {
    console.error(`order ${i}: ${answer.status}`);
    process.exit(1);
  }
  await answer.text();
  appendFileSync("/stand/out/orders.jsonl", `${JSON.stringify({ i, status: answer.status })}\n`);
}
"#;

const PING: &str = r#"fetch("http://localhost:3000/health").then((r) => process.exit(r.ok ? 0 : 1), () => process.exit(1));
"#;

const PACKAGE: &str = r#"{ "name": "stand-live", "private": true, "scripts": { "build": "node build.js" }, "dependencies": { "pg": "8.13.1" } }
"#;

const BUILD: &str = r#"// Stands in for a compiler: `dist/` is what the services run.
const fs = require("node:fs");
fs.mkdirSync("dist", { recursive: true });
for (const file of fs.readdirSync("src")) fs.copyFileSync(`src/${file}`, `dist/${file}`);
"#;

const MAIN: &str = r#"const http = require("node:http");
const { Pool } = require("pg");

const pool = new Pool({ host: "db", user: "postgres", password: "stand", database: "postgres", max: 4 });

async function order() {
  const { rows } = await pool.query("insert into orders (item) values ($1) returning id", ["book"]);
  return rows[0];
}

const server = http.createServer((request, response) => {
  if (request.url === "/health") return response.end("ok");
  if (request.method === "POST" && request.url === "/orders") {
    return order().then(
      (row) => response.end(JSON.stringify(row)),
      (error) => { console.error(error); response.statusCode = 500; response.end(); },
    );
  }
  response.statusCode = 404;
  response.end();
});
server.listen(3000);
process.on("SIGTERM", () => server.close(() => pool.end().then(() => process.exit(0))));
"#;

const MIGRATE: &str = r#"const { Client } = require("pg");
const client = new Client({ host: "db", user: "postgres", password: "stand", database: "postgres" });
client.connect()
  .then(() => client.query("create table if not exists orders (id serial primary key, item text not null, at timestamptz not null default now())"))
  .then(() => client.end())
  .catch((error) => { console.error(error); process.exit(1); });
"#;

/// The head's change: one more lookup per order.
const LOOKUP: &str =
    "  await pool.query(\"select count(*) from orders where item = $1\", [\"book\"]);\n";

/// Two services, no setup: `db` starts empty in each repeat, and the workload fills a table.
const SMALL: &str = r#"
services:
  db:
    image: postgres:17-alpine
    environment: { POSTGRES_PASSWORD: stand }
    volumes: ["pgdata:/var/lib/postgresql/data"]
    expose: [5432]
    healthcheck: { test: ["CMD", "pg_isready", "-U", "postgres", "-h", "127.0.0.1"], interval: 1s, retries: 60 }
    deploy: { resources: { limits: { cpus: "1", memory: 256m } } }
  load:
    image: postgres:17-alpine
    environment: { PGPASSWORD: stand }
    command: ["psql", "-h", "db", "-U", "postgres", "-v", "ON_ERROR_STOP=1", "-c", "create table hits (n int primary key)", "-c", "insert into hits select generate_series(1, 20)"]
    depends_on: { db: { condition: service_healthy } }
    deploy: { resources: { limits: { cpus: "1", memory: 128m } } }
volumes: { pgdata: {} }
x-stand:
  workload: load
  repeat: 2
  setup_timeout: 2m
  timeout: 1m
  probes:
    hits:
      in: db
      at: after-workload
      run: ["psql", "-U", "postgres", "-At", "-c", "select row_to_json(h) from hits h order by n"]
      key: /n
"#;

fn live() -> bool {
    std::env::var("STOP_SLOP_STAND_LIVE").as_deref() == Ok("1")
}

/// Its own stand folder, kept between runs so the runtime downloads once; `clean` drops the
/// images. Under `/tmp` on Unix, so the sockets of microsandbox fit `sun_path`.
fn home() -> Utf8PathBuf {
    if cfg!(unix) {
        Utf8PathBuf::from("/tmp/stop-slop-stand-cli")
    } else {
        let temp = Utf8PathBuf::from_path_buf(std::env::temp_dir()).unwrap();
        temp.join("stop-slop-stand-cli")
    }
}

/// The adapter on the binary's microsandbox home, one per process as the SDK wants: it reads the
/// namespace between the binary's runs.
fn adapter() -> &'static Microsandbox {
    static ADAPTER: OnceLock<Microsandbox> = OnceLock::new();
    ADAPTER.get_or_init(|| Microsandbox::new(&home().join("msb")).expect("the adapter"))
}

/// A line of `<home>/notes.txt` for the PR description, which starts empty in each process.
fn note(line: &str) {
    static FRESH: Once = Once::new();
    FRESH.call_once(|| {
        let _ = fs::remove_file(home().join("notes.txt"));
    });
    println!("{line}");
    let os = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
    let mut notes = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(home().join("notes.txt"))
        .unwrap();
    writeln!(notes, "{os}: {line}").unwrap();
}

struct Ran {
    exit: Option<i32>,
    out: String,
    err: String,
}

struct Stand {
    repo: TempRepo,
}

impl Stand {
    /// A committed repo of `files` with the recipe of `harness` in `.git/info/stop-slop-stand/`.
    fn new(files: &[(&str, &str)], harness: &[(&str, &str)]) -> Stand {
        fs::create_dir_all(home()).unwrap();
        let repo = TempRepo::new();
        for (path, text) in files {
            repo.write(path, text);
        }
        repo.commit("app");
        let stand = Stand { repo };
        fs::create_dir_all(stand.recipe()).unwrap();
        for (name, text) in harness {
            fs::write(stand.recipe().join(name), text).unwrap();
        }
        stand
    }

    fn cwd(&self) -> &Utf8Path {
        Utf8Path::from_path(self.repo.path()).unwrap()
    }

    fn recipe(&self) -> Utf8PathBuf {
        self.cwd().join(".git/info/stop-slop-stand")
    }

    /// Through pipes, as an agent's shell reads it: a VM process that kept one open would hang it.
    fn run(&self, args: &[&str]) -> Ran {
        let started = Instant::now();
        let Output {
            status,
            stdout,
            stderr,
        } = Command::new(BIN)
            .args(args)
            .current_dir(self.cwd())
            .env("STOP_SLOP_STAND_HOME", home())
            .output()
            .expect("start stop-slop-stand");
        let took = started.elapsed();
        note(&format!(
            "{}: exit {:?} in {:.1} s",
            args.join(" "),
            status.code(),
            took.as_secs_f64()
        ));
        Ran {
            exit: status.code(),
            out: String::from_utf8_lossy(&stdout).into_owned(),
            err: String::from_utf8_lossy(&stderr).into_owned(),
        }
    }

    fn passes(&self, args: &[&str]) -> Ran {
        let ran = self.run(args);
        assert_eq!(ran.exit, Some(0), "{args:?}\n{}{}", ran.out, ran.err);
        ran
    }

    /// The one line of `--json`, with its exit code.
    fn line(&self, args: &[&str], code: i32) -> Value {
        let ran = self.run(args);
        assert_eq!(ran.exit, Some(code), "{args:?}\n{}{}", ran.out, ran.err);
        let lines: Vec<&str> = ran.out.lines().collect();
        assert_eq!(lines.len(), 1, "one line: {}{}", ran.out, ran.err);
        note(&format!("  line {}", lines[0]));
        if !ran.err.is_empty() {
            note(&format!(
                "  stderr {}",
                ran.err.trim_end().replace('\n', " | ")
            ));
        }
        serde_json::from_str(lines[0]).unwrap()
    }

    /// `clean`, `snapshot`, then `edit` uncommitted: the session's "before" and a head after it.
    fn session(&self, edit: impl FnOnce(&Utf8Path)) -> String {
        // Whatever an earlier run of this folder left: the namespace starts empty.
        self.passes(&["clean"]);
        self.passes(&["snapshot"]);
        edit(self.cwd());
        self.repo.git(&["status", "--porcelain"])
    }

    /// `clean` drops every VM, snapshot and image of the namespace.
    fn clean(&self, images: &[&str]) {
        self.passes(&["clean"]);
        let left = adapter().leftovers().unwrap();
        assert!(
            left.sandboxes.is_empty() && left.snapshots.is_empty(),
            "{left:?}"
        );
        let images: Vec<ImageRef> = images.iter().copied().map(ImageRef::new).collect();
        let cached = adapter().cached(&images).unwrap();
        assert!(
            cached.digests.iter().all(|(_, digest)| digest.is_none()),
            "{cached:?}"
        );
    }
}

fn read_json(path: &Utf8Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|error| panic!("{path}: {error}"));
    serde_json::from_str(&text).unwrap()
}

fn report_dir(line: &Value) -> Utf8PathBuf {
    let report = Utf8Path::new(line["report"].as_str().expect("a report"));
    report.parent().unwrap().to_owned()
}

fn sessions() -> Vec<Utf8PathBuf> {
    let entries = fs::read_dir(home().join("sessions")).into_iter().flatten();
    let dirs = entries.flatten().map(|entry| entry.path());
    dirs.map(|dir| Utf8PathBuf::from_path_buf(dir).unwrap())
        .collect()
}

/// The snapshots of each session's kept run and of its cache entries.
fn named_snapshots() -> BTreeSet<String> {
    let mut named = Vec::<SnapshotId>::new();
    for session in sessions() {
        if let Some(run) = read_json(&session.join("session.json"))["kept"].as_str() {
            named.extend(run::kept_ids(&session.join("runs").join(run)).unwrap());
        }
        for entry in fs::read_dir(session.join("cache"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let entry = Utf8PathBuf::from_path_buf(entry.path()).unwrap();
            named.extend(run::cached_ids(&entry).unwrap().unwrap_or_default());
        }
    }
    named.iter().map(ToString::to_string).collect()
}

/// No VM is left, and the namespace holds exactly the named snapshots.
fn only_named_snapshots_left() -> usize {
    let left = adapter().leftovers().unwrap();
    assert!(left.sandboxes.is_empty(), "{left:?}");
    let snapshots: BTreeSet<String> = left.snapshots.iter().map(ToString::to_string).collect();
    assert_eq!(snapshots, named_snapshots());
    snapshots.len()
}

/// `key.guest` of each cache slot: the pin, unless `STOP_SLOP_STAND_GUEST` names a build.
fn slot_guests() -> Vec<String> {
    let mut guests = Vec::new();
    for session in sessions() {
        let state = read_json(&session.join("session.json"));
        for slot in ["head", "before"] {
            if let Some(guest) = state["cache"][slot]["key"]["guest"].as_str() {
                guests.push(guest.to_owned());
            }
        }
    }
    let dev = std::env::var_os("STOP_SLOP_STAND_GUEST").is_some();
    assert!(!guests.is_empty());
    assert!(
        guests.iter().all(|guest| guest.starts_with("dev-") == dev),
        "{guests:?}"
    );
    guests
}

/// Any `node_modules` under the stand's folder: the install belongs on the version disk.
fn node_modules(dir: &Utf8Path) -> Option<Utf8PathBuf> {
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = Utf8PathBuf::from_path_buf(entry.path()).ok()?;
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            if path.file_name() == Some("node_modules") {
                return Some(path);
            }
            if let Some(found) = node_modules(&path) {
                return Some(found);
            }
        }
    }
    None
}

fn kinds(flags: &Value) -> Vec<&str> {
    let flags = flags.as_array().into_iter().flatten();
    flags.filter_map(|flag| flag["kind"].as_str()).collect()
}

#[test]
fn the_stand_measures_a_change_against_its_session_and_cleans_up() {
    if !live() {
        return;
    }
    let started = Instant::now();
    let stand = Stand::new(
        &[
            ("package.json", PACKAGE),
            ("build.js", BUILD),
            ("src/main.js", MAIN),
            ("src/migrate.js", MIGRATE),
            (".gitignore", "node_modules/\ndist/\n"),
        ],
        &[
            ("compose.yaml", RECIPE),
            ("load.mjs", LOAD),
            ("ping.js", PING),
        ],
    );
    let status = stand.session(|cwd| {
        let main = cwd.join("src/main.js");
        let text = fs::read_to_string(&main).unwrap();
        let changed = text.replace(
            "  return rows[0];\n",
            &format!("{LOOKUP}  return rows[0];\n"),
        );
        fs::write(&main, changed).unwrap();
    });

    // `clean` dropped the images, and nothing was built yet.
    let dry = stand
        .passes(&["run", "--before", "session", "--dry-run"])
        .out;
    for expected in [
        format!("image {POSTGRES} to download"),
        format!("image {NODE} to download"),
        "setup head to build".to_owned(),
        "setup before to build".to_owned(),
    ] {
        assert!(
            dry.lines().any(|line| line == expected),
            "{expected}: {dry}"
        );
    }

    let first = stand.line(&["run", "--before", "session", "--json"], 0);
    let run = first["run"].as_str().unwrap().to_owned();
    let run_dir = report_dir(&first);
    let report = read_json(&run_dir.join("report.json"));
    assert_eq!(
        stand.repo.git(&["status", "--porcelain"]),
        status,
        "the work tree stays as it was"
    );
    assert_eq!(
        node_modules(&home()),
        None,
        "no install lands in the stand's folder"
    );
    let orders = fs::read_to_string(run_dir.join("repeat-1-head/out/probe.orders.jsonl")).unwrap();
    assert_eq!(orders.lines().count(), 50, "{orders}");
    assert!(report["head"]["setup"]["cached"].is_null());
    assert!(report["before"]["setup"]["cached"].is_null());
    let open = &report["secondPass"]["open"];
    let flags = kinds(&report["flags"]);
    note(&format!("  secondPass.open {open}, flags {flags:?}"));
    // Each service called exposes its port, and the pool of `api` keeps the same connections to
    // `db` from one pass to the next: neither is a finding.
    assert!(
        !flags.contains(&"missing-expose") && !flags.contains(&"connections-grow"),
        "{flags:?}"
    );
    note(&format!("  guest {:?}", slot_guests()));
    let snapshots = only_named_snapshots_left();

    let inspect = [
        "inspect",
        &run,
        "head",
        "db",
        "--",
        "psql",
        "-U",
        "postgres",
        "-At",
        "-c",
        "select count(*) from orders",
    ];
    let looked = Instant::now();
    let out = stand.passes(&inspect).out;
    let lines: Vec<&str> = out.lines().collect();
    // The end of the last head repeat, which started from the setup snapshot.
    assert_eq!(lines.get(..2), Some(&["exit 0", "50"][..]), "{out}");
    assert!(lines.last().unwrap().starts_with("full output "), "{out}");
    assert!(looked.elapsed() < Duration::from_secs(30));
    assert_eq!(
        adapter().leftovers().unwrap().snapshots.len(),
        snapshots,
        "inspect changes no snapshot"
    );

    let dry = stand
        .passes(&["run", "--before", "session", "--dry-run"])
        .out;
    for image in [POSTGRES, NODE] {
        let cached = format!("image {image} cached sha256:");
        assert!(
            dry.lines().any(|line| line.starts_with(&cached)),
            "{cached}: {dry}"
        );
    }
    for side in ["head", "before"] {
        let cached = format!("setup {side} cached {run}");
        assert!(dry.lines().any(|line| line == cached), "{cached}: {dry}");
    }

    let second = stand.line(&["run", "--before", "session", "--json"], 0);
    assert_eq!(second["attempts"], 2);
    let second_dir = report_dir(&second);
    let again = read_json(&second_dir.join("report.json"));
    assert_eq!(
        again["before"]["setup"]["cached"].as_str(),
        Some(run.as_str()),
        "before from the cache"
    );
    assert_eq!(
        again["head"]["setup"]["cached"].as_str(),
        Some(run.as_str()),
        "so the head: no edit"
    );
    assert_eq!(
        again["coverage"]["from"].as_str(),
        Some(run.as_str()),
        "no coverage repeat"
    );
    assert_eq!(
        again["secondPass"]["from"].as_str(),
        Some(run.as_str()),
        "no second pass"
    );
    assert!(!second_dir.join("coverage").exists() && !second_dir.join("pass2-head").exists());
    let flags = kinds(&again["flags"]);
    note(&format!("  flags {flags:?}"));

    fs::write(
        stand.recipe().join("load.mjs"),
        format!("{LOAD}// edited\n"),
    )
    .unwrap();
    let changed = stand.line(&["run", "--before", "session", "--json"], 2);
    assert_eq!(changed["failure"]["phase"], "seal");
    assert!(
        changed["failure"]["cause"]
            .as_str()
            .unwrap()
            .contains("load.mjs"),
        "{changed}"
    );
    assert!(
        changed["next"][0].as_str().unwrap().contains("--reseal"),
        "{changed}"
    );

    stand.clean(&[POSTGRES, NODE]);
    note(&format!(
        "the whole case: {:.0} s",
        started.elapsed().as_secs_f64()
    ));
}

#[test]
fn a_two_service_stand_passes_on_four_cpus_and_clean_leaves_nothing() {
    if !live() {
        return;
    }
    let started = Instant::now();
    let stand = Stand::new(&[("README.md", "v1\n")], &[("compose.yaml", SMALL)]);
    stand.session(|cwd| fs::write(cwd.join("README.md"), "v2\n").unwrap());

    let line = stand.line(&["run", "--before", "session", "--json"], 0);
    let hits =
        fs::read_to_string(report_dir(&line).join("repeat-1-head/out/probe.hits.jsonl")).unwrap();
    assert_eq!(hits.lines().count(), 20, "{hits}");
    let flags = kinds(&line["flags"]);
    assert!(
        !flags.contains(&"missing-expose"),
        "nothing calls load: {flags:?}"
    );
    note(&format!("  guest {:?}", slot_guests()));
    only_named_snapshots_left();

    stand.clean(&[POSTGRES]);
    note(&format!(
        "the small case: {:.0} s",
        started.elapsed().as_secs_f64()
    ));
}
