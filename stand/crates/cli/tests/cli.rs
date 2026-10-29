//! `execute` on a `Fake`, a temporary stand folder and a `testkit` repo whose stand file lies in
//! `<git common>/info/stop-slop-stand/`. The host is a value of the test: a CI runner has two cores
//! and less than the 20 GB the stand keeps free.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::time::{Duration, Instant, SystemTime};

use budget::{Host, HostError};
use camino::{Utf8Path, Utf8PathBuf};
use cli::{Env, Exit};
use run::Cancel;
use sandbox::{
    Cached, Call, ExecId, ExecOutcome, Exited, Fake, Fault, ImageRef, Launch, Leftovers, Metrics,
    Pulled, Rule, Sandbox, SandboxError, SandboxId, Scenario, Script, SnapshotId, Stopped,
    VolumeName, Waitable,
};
use serde_json::Value;
use store::{Store, StorePaths};
use tempfile::TempDir;
use testkit::TempRepo;

const GIB: u64 = 1 << 30;
const SECOND: Duration = Duration::from_secs(1);

/// `db` with a volume and a probe, the prepare service, a profiled `api` from its snapshot, the
/// workload `load` from the harness.
const RECIPE: &str = r#"
services:
  db:
    image: postgres:17
    volumes: ["pgdata:/var/lib/postgresql/data"]
    expose: [5432]
    healthcheck: { test: ["CMD", "pg_isready"], interval: 1s, retries: 3 }
    deploy: { resources: { limits: { cpus: "1", memory: 256m } } }
  prepare:
    image: node:22
    working_dir: /w/app
    volumes: ["${STAND_REPO}:/w/app"]
    command: ["npm", "ci", "--cache", "/w/.npm"]
    deploy: { resources: { limits: { cpus: "1", memory: 256m } } }
  api:
    image: node:22
    working_dir: /w/app
    volumes: ["${STAND_REPO}:/w/app"]
    command: ["node", "main.js"]
    expose: [3000]
    depends_on:
      prepare: { condition: service_completed_successfully }
      db: { condition: service_healthy }
    healthcheck: { test: ["CMD", "node", "ping.js"], interval: 1s, retries: 3 }
    deploy: { resources: { limits: { cpus: "1", memory: 256m } } }
    x-stand: { profile: node }
  load:
    image: node:22
    working_dir: /harness
    volumes: [".:/harness:ro"]
    command: ["node", "load.js"]
    depends_on: { api: { condition: service_healthy } }
    deploy: { resources: { limits: { cpus: "1", memory: 128m } } }
volumes: { pgdata: {} }
x-stand:
  workload: load
  repeat: 2
  probes:
    orders: { in: db, at: after-workload, run: ["psql", "-c", "select 1"], key: /id }
"#;

/// One coverage chunk of a Node process of `api`: three calls of `order`.
const COVERAGE_CHUNK: &str = r#"[{"url":"file:///w/app/main.js","functions":[{"functionName":"order","ranges":[{"count":3}],"line":4,"column":15}]}]"#;

fn roomy(_: &Utf8Path) -> Result<Host, HostError> {
    Ok(Host {
        cpus: 64,
        memory: 64 * GIB,
        free_memory: 64 * GIB,
        free_disk: 1024 * GIB,
    })
}

/// A repo with a stand file, and a stand folder of its own.
struct World {
    dir: TempDir,
    repo: TempRepo,
    cwd: Utf8PathBuf,
    paths: StorePaths,
    store: Store,
}

struct Ran {
    exit: Exit,
    out: String,
    err: String,
}

impl Ran {
    fn new(exit: Exit, out: Vec<u8>, err: Vec<u8>) -> Ran {
        Ran {
            exit,
            out: String::from_utf8(out).unwrap(),
            err: String::from_utf8(err).unwrap(),
        }
    }

    /// The one line of `--json`.
    fn line(&self) -> Value {
        let lines: Vec<&str> = self.out.lines().collect();
        assert_eq!(lines.len(), 1, "one line: {}{}", self.out, self.err);
        serde_json::from_str(lines[0]).unwrap()
    }

    /// The `next:` lines at the end of stderr.
    fn next(&self) -> Vec<&str> {
        self.err
            .lines()
            .filter_map(|line| line.strip_prefix("next: "))
            .collect()
    }
}

impl World {
    fn new() -> World {
        World::with(RECIPE)
    }

    fn with(recipe: &str) -> World {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(dir.path()).unwrap().to_owned();
        let repo = TempRepo::new();
        repo.write("main.js", "console.log('v1')\n")
            .write("package.json", "{}\n")
            .commit("app");
        let cwd = Utf8Path::from_path(repo.path()).unwrap().to_owned();
        let paths = StorePaths {
            root: root.join("stand"),
            config: root.join("stand.toml"),
        };
        let store = Store::open(paths.clone()).unwrap();
        let world = World {
            dir,
            repo,
            cwd,
            paths,
            store,
        };
        if !recipe.is_empty() {
            world.write_recipe("compose.yaml", recipe);
            world.write_recipe("load.js", "// load\n");
            world.write_recipe("ping.js", "// ping\n");
        }
        world
    }

    fn recipe(&self) -> Utf8PathBuf {
        self.cwd.join(".git/info/stop-slop-stand")
    }

    fn write_recipe(&self, name: &str, text: &str) {
        fs::create_dir_all(self.recipe()).unwrap();
        fs::write(self.recipe().join(name), text).unwrap();
    }

    fn tar(&self, side: &str) -> Utf8PathBuf {
        self.paths.root.join("trees").join(format!("{side}.tar"))
    }

    fn run(&self, sandbox: &impl Sandbox, args: &[&str]) -> Ran {
        self.run_on(sandbox, args, &roomy, &Cancel::new())
    }

    fn run_on(
        &self,
        sandbox: &impl Sandbox,
        args: &[&str],
        host: &dyn Fn(&Utf8Path) -> Result<Host, HostError>,
        cancel: &Cancel,
    ) -> Ran {
        self.run_at(&self.cwd, sandbox, args, host, cancel)
    }

    /// A linked work tree of the repo: another session of the same stand file and stand folder.
    fn worktree(&self, name: &str) -> Utf8PathBuf {
        let path = Utf8Path::from_path(self.dir.path()).unwrap().join(name);
        self.repo
            .git(&["worktree", "add", "-q", "--detach", path.as_str()]);
        path
    }

    fn run_at(
        &self,
        cwd: &Utf8Path,
        sandbox: &impl Sandbox,
        args: &[&str],
        host: &dyn Fn(&Utf8Path) -> Result<Host, HostError>,
        cancel: &Cancel,
    ) -> Ran {
        let args: Vec<OsString> = std::iter::once("stop-slop-stand")
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let exit = match cli::parse(args.iter().cloned()) {
            Err(error) => cli::refuse_args(&error, &args, &mut out, &mut err),
            Ok(command) => cli::execute(
                command,
                sandbox,
                Env {
                    cwd,
                    store: &self.store,
                    cancel,
                    host,
                    out: &mut out,
                    err: &mut err,
                },
            ),
        };
        Ran::new(exit, out, err)
    }

    fn report(&self, line: &Value) -> Value {
        read_json(Utf8Path::new(line["report"].as_str().expect("a report")))
    }

    /// The folder of a run by its id, in whichever session holds it.
    fn run_dir(&self, run: &str) -> Utf8PathBuf {
        self.sessions()
            .into_iter()
            .map(|session| session.join("runs").join(run))
            .find(|dir| dir.is_dir())
            .unwrap_or_else(|| panic!("no folder of run {run}"))
    }

    /// `sessions/<key>/` of `store`, sorted.
    fn sessions(&self) -> Vec<Utf8PathBuf> {
        folders(&self.paths.root.join("sessions"))
    }
}

fn read_json(path: &Utf8Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// What a run did from `mark` of the journal up to its first `Create`: the start.
fn before_first_create(fake: &Fake, mark: usize) -> Vec<Call> {
    let journal = fake.journal()[mark..].to_vec();
    let first = journal
        .iter()
        .position(|call| matches!(call, Call::Create { .. }))
        .unwrap();
    journal[..first].to_vec()
}

fn dropped(calls: &[Call]) -> Vec<SnapshotId> {
    let ids = calls.iter().filter_map(|call| match call {
        Call::DropSnapshot(id) => Some(id.clone()),
        _ => None,
    });
    ids.collect()
}

fn folders(dir: &Utf8Path) -> Vec<Utf8PathBuf> {
    let Ok(entries) = dir.read_dir_utf8() else {
        return Vec::new();
    };
    let mut found: Vec<Utf8PathBuf> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.path().to_owned())
        .collect();
    found.sort();
    found
}

/// The setup cache entries of a session (`sessions/<key>/cache/<digest>/`, `store`).
fn entries_of(session: &Utf8Path) -> Vec<Utf8PathBuf> {
    folders(&session.join("cache"))
}

fn entries(world: &World) -> Vec<Utf8PathBuf> {
    world
        .sessions()
        .iter()
        .flat_map(|s| entries_of(s))
        .collect()
}

/// The snapshots the cache entries of a session list.
fn cached_of(session: &Utf8Path) -> Vec<SnapshotId> {
    let lists = entries_of(session).into_iter();
    let lists = lists.map(|entry| run::cached_ids(&entry).unwrap().unwrap_or_default());
    lists.flatten().collect()
}

/// The snapshots of the kept run of a session (`kept` of `session.json`, `kept.json`).
fn kept_of(session: &Utf8Path) -> Vec<SnapshotId> {
    let state = read_json(&session.join("session.json"));
    match state["kept"].as_str() {
        Some(run) => run::kept_ids(&session.join("runs").join(run)).unwrap(),
        None => Vec::new(),
    }
}

fn rule(service: &str, script: Script) -> Rule {
    Rule {
        service: service.into(),
        tree: None,
        script,
    }
}

fn on_tree(service: &str, tree: Utf8PathBuf, script: Script) -> Rule {
    Rule {
        service: service.into(),
        tree: Some(tree),
        script,
    }
}

fn exits(code: i32, after: Duration) -> Script {
    Script {
        exit: Some((code, after)),
        ..Script::default()
    }
}

fn healthy() -> Script {
    Script {
        health: vec![Some(0)],
        stop: Stopped {
            after: Duration::from_millis(100),
            killed: false,
        },
        ..Script::default()
    }
}

/// Everything passes; `extra` rules come first and win.
fn fake(extra: Vec<Rule>) -> Fake {
    let mut rules = extra;
    rules.extend([
        rule(
            "db",
            Script {
                volume_bytes: 4096,
                probes: vec![(
                    "probe.orders.jsonl".into(),
                    ExecOutcome::Exited(0),
                    b"{\"id\":1}\n".to_vec(),
                )],
                ..healthy()
            },
        ),
        rule("prepare", exits(0, 5 * SECOND)),
        rule("api", healthy()),
        rule("load", exits(0, 10 * SECOND)),
    ]);
    Fake::new(Scenario {
        rules,
        ..Scenario::default()
    })
}

/// An `api` whose Node process leaves a coverage chunk in `/stand/profile`.
fn profiled_api() -> Rule {
    let writes = [
        ("1.start.json", "{}"),
        ("1.0.coverage.json", COVERAGE_CHUNK),
    ];
    let writes =
        writes.map(|(name, text)| (format!("/stand/profile/{name}"), text.as_bytes().to_vec()));
    rule(
        "api",
        Script {
            writes: writes.to_vec(),
            ..healthy()
        },
    )
}

fn creates(fake: &Fake, service: &str) -> usize {
    fake.journal()
        .iter()
        .map(|call| match call {
            Call::Create { batch, .. } => batch
                .iter()
                .filter(|launch| launch.service == service)
                .count(),
            _ => 0,
        })
        .sum()
}

fn any_create(fake: &Fake) -> bool {
    fake.journal()
        .iter()
        .any(|call| matches!(call, Call::Create { .. }))
}

fn faults(fake: &Fake) -> usize {
    let journal = fake.journal();
    journal
        .iter()
        .filter(|call| matches!(call, Call::Fault { .. }))
        .count()
}

fn snapshots_left(fake: &Fake) -> Vec<SnapshotId> {
    fake.leftovers().unwrap().snapshots
}

fn set(ids: &[SnapshotId]) -> BTreeSet<String> {
    ids.iter().map(ToString::to_string).collect()
}

/// The `Fake` of a case, with the calls it changes: `pull` and `cached` give the digests of
/// `digests` (a tag that moved), `cached` finds none of `evicted`, a `create` of a batch with the
/// service `fails_on` fails, one with `cancels_on` sets Ctrl+C first, `after_exec` gets the
/// `stdout` of each `exec` once it is written, as the guest's `argv` could change it, and a
/// `drop_snapshot` of one of `undroppable` fails. Everything else goes to the `Fake` as it is.
struct Wrapped<'f> {
    fake: &'f Fake,
    digests: &'f [(&'f str, &'f str)],
    evicted: &'f [&'f str],
    fails_on: Option<&'f str>,
    cancels_on: Option<(&'f str, &'f Cancel)>,
    after_exec: Option<&'f dyn Fn(&Utf8Path)>,
    undroppable: &'f [SnapshotId],
}

impl<'f> Wrapped<'f> {
    fn new(fake: &'f Fake) -> Wrapped<'f> {
        Wrapped {
            fake,
            digests: &[],
            evicted: &[],
            fails_on: None,
            cancels_on: None,
            after_exec: None,
            undroppable: &[],
        }
    }

    fn digest(&self, image: &ImageRef) -> Option<String> {
        let listed = self
            .digests
            .iter()
            .find(|(name, _)| *name == image.as_str());
        listed.map(|(_, digest)| (*digest).to_owned())
    }
}

fn holds(batch: &[Launch], service: &str) -> bool {
    batch.iter().any(|launch| launch.service == service)
}

impl Sandbox for Wrapped<'_> {
    fn now(&self) -> Instant {
        self.fake.now()
    }
    fn pull(&self, images: &[ImageRef]) -> Result<Pulled, SandboxError> {
        let mut pulled = self.fake.pull(images)?;
        for (image, digest) in &mut pulled.digests {
            if let Some(moved) = self.digest(image) {
                *digest = moved;
            }
        }
        Ok(pulled)
    }
    fn cached(&self, images: &[ImageRef]) -> Result<Cached, SandboxError> {
        let mut cached = self.fake.cached(images)?;
        for (image, digest) in &mut cached.digests {
            if self.evicted.contains(&image.as_str()) {
                *digest = None;
            } else if let Some(moved) = self.digest(image) {
                *digest = Some(moved);
            }
        }
        Ok(cached)
    }
    fn create(&self, batch: &[Launch]) -> Result<Vec<SandboxId>, SandboxError> {
        if self.fails_on.is_some_and(|service| holds(batch, service)) {
            return Err(SandboxError::Backend {
                message: "the VMM went away".into(),
            });
        }
        if let Some((service, cancel)) = self.cancels_on
            && holds(batch, service)
        {
            cancel.cancel();
        }
        self.fake.create(batch)
    }
    fn exec_detached(
        &self,
        id: &SandboxId,
        argv: &[String],
        timeout: Duration,
    ) -> Result<ExecId, SandboxError> {
        self.fake.exec_detached(id, argv, timeout)
    }
    fn wait(&self, what: &[Waitable], deadline: Instant) -> Result<Option<Exited>, SandboxError> {
        self.fake.wait(what, deadline)
    }
    fn exec(
        &self,
        id: &SandboxId,
        argv: &[String],
        deadline: Instant,
        stdout: &Utf8Path,
    ) -> Result<ExecOutcome, SandboxError> {
        let outcome = self.fake.exec(id, argv, deadline, stdout)?;
        if let Some(after) = self.after_exec {
            after(stdout);
        }
        Ok(outcome)
    }
    fn stop(&self, id: &SandboxId) -> Result<Stopped, SandboxError> {
        self.fake.stop(id)
    }
    fn fault(&self, ids: &[SandboxId], fault: &Fault) -> Result<(), SandboxError> {
        self.fake.fault(ids, fault)
    }
    fn metrics(&self, id: &SandboxId) -> Result<Metrics, SandboxError> {
        self.fake.metrics(id)
    }
    fn logs(&self, id: &SandboxId) -> Result<Vec<u8>, SandboxError> {
        self.fake.logs(id)
    }
    fn copy_out(&self, id: &SandboxId, guest: &str, host: &Utf8Path) -> Result<(), SandboxError> {
        self.fake.copy_out(id, guest, host)
    }
    fn volume_sizes(&self, id: &SandboxId) -> Result<Vec<(VolumeName, u64)>, SandboxError> {
        self.fake.volume_sizes(id)
    }
    fn snapshot(&self, id: &SandboxId) -> Result<SnapshotId, SandboxError> {
        self.fake.snapshot(id)
    }
    fn destroy(&self, id: &SandboxId) -> Result<(), SandboxError> {
        self.fake.destroy(id)
    }
    fn drop_snapshot(&self, snapshot: &SnapshotId) -> Result<(), SandboxError> {
        if self.undroppable.contains(snapshot) {
            return Err(SandboxError::Backend {
                message: "the snapshot store went away".into(),
            });
        }
        self.fake.drop_snapshot(snapshot)
    }
    fn leftovers(&self) -> Result<Leftovers, SandboxError> {
        self.fake.leftovers()
    }
    fn prune_images(&self, keep: &[ImageRef]) -> Result<(), SandboxError> {
        self.fake.prune_images(keep)
    }
}

#[test]
fn a_run_passes_with_its_report_and_leaves_no_tar_and_no_lock() {
    let world = World::new();
    let fake = fake(Vec::new());
    let ran = world.run(&fake, &["run"]);
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    let report = ran
        .out
        .lines()
        .find_map(|line| line.strip_prefix("report "))
        .expect("the path of the report");
    assert!(Utf8Path::new(report).is_file(), "{report}");
    assert!(!report.contains('\\'), "{report}");
    assert!(!world.tar("head").exists() && !world.tar("before").exists());
    assert!(
        world.store.try_lock().unwrap().is_some(),
        "the lock is free"
    );
    assert!(fake.leftovers().unwrap().sandboxes.is_empty());
}

#[test]
fn the_json_line_is_one_line_on_success_failure_and_refusal_and_pretty_indents_it() {
    let world = World::new();
    let passed = world.run(&fake(Vec::new()), &["run", "--json"]).line();
    assert!(
        passed["run"].is_string() && passed["report"].is_string() && passed["failure"].is_null()
    );
    let failed = world.run(
        &fake(vec![rule("api", exits(1, 2 * SECOND))]),
        &["run", "--json"],
    );
    assert_eq!(failed.exit, Exit::AppFailed);
    assert!(failed.line()["report"].is_null() && failed.line()["run"].is_string());
    let refused = World::with("").run(&fake(Vec::new()), &["run", "--json"]);
    assert_eq!(refused.exit, Exit::Refused);
    let line = refused.line();
    assert_eq!(line["failure"]["phase"], "spec");
    assert_eq!(line["next"], serde_json::json!(["stop-slop-stand init"]));
    assert_eq!(refused.next(), ["stop-slop-stand init"]);
    // A flag `clap` refuses: the line of the phase `args`, as `check --json` of the linter.
    let flag = world.run(&fake(Vec::new()), &["run", "--json", "--fast"]);
    assert_eq!(flag.exit, Exit::Refused);
    let line = flag.line();
    assert_eq!(line["failure"]["phase"], "args");
    assert!(
        line["failure"]["cause"]
            .as_str()
            .unwrap()
            .contains("--fast"),
        "{line}"
    );
    assert_eq!(line["next"], serde_json::json!(["stop-slop-stand guide"]));
    assert_eq!(flag.next(), ["stop-slop-stand guide"]);
    let pretty = world.run(&fake(Vec::new()), &["run", "--json", "--pretty"]);
    let value: Value = serde_json::from_str(&pretty.out).unwrap();
    assert!(pretty.out.lines().count() > 1 && value["run"].is_string());
    // Before `execute`: the folders of the OS, the stand folder or the adapter fail in `main`.
    for (args, json) in [
        (&["run", "--json"][..], true),
        (&["run", "--dry-run", "--json"], true),
        (&["run"], false),
        (&["guide"], false),
    ] {
        let args = std::iter::once("stop-slop-stand").chain(args.iter().copied());
        let command = cli::parse(args.map(OsString::from)).unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let exit = cli::refuse_start(&command, "C:/stand: denied\nby the OS", &mut out, &mut err);
        let ran = Ran::new(exit, out, err);
        assert_eq!(ran.exit, Exit::Broken);
        assert!(
            ran.err.starts_with("C:/stand: denied\nby the OS\n"),
            "{}",
            ran.err
        );
        assert_eq!(ran.next(), ["stop-slop-stand guide"]);
        if json {
            let line = ran.line();
            assert_eq!(line["failure"]["phase"], "config");
            assert_eq!(line["failure"]["cause"], "C:/stand: denied; by the OS");
            assert_eq!(line["next"], serde_json::json!(["stop-slop-stand guide"]));
        } else {
            assert_eq!(ran.out, "");
        }
    }
}

#[test]
fn refusals_before_the_start_are_2_with_their_next_and_no_vm() {
    let wide = RECIPE.replace("cpus: \"1\", memory: 128m", "cpus: \"1000\", memory: 128m");
    assert_ne!(wide, RECIPE);
    let missing = || {
        Fake::new(Scenario {
            missing_images: vec!["postgres:17".into()],
            ..Scenario::default()
        })
    };
    let cases: Vec<(World, &[&str], Fake, &str, &str)> = vec![
        (
            World::with(""),
            &[],
            fake(Vec::new()),
            "spec",
            "stop-slop-stand init",
        ),
        (
            World::with("services: {}\n"),
            &[],
            fake(Vec::new()),
            "spec",
            "stop-slop-stand guide recipe",
        ),
        (
            World::with(&wide),
            &[],
            fake(Vec::new()),
            "budget",
            "stop-slop-stand guide",
        ),
        (
            World::new(),
            &["--before", "session"],
            fake(Vec::new()),
            "source",
            "stop-slop-stand snapshot",
        ),
        (
            World::new(),
            &["--before", "no-such-ref"],
            fake(Vec::new()),
            "source",
            "stop-slop-stand guide",
        ),
        (
            World::new(),
            &[],
            missing(),
            "pull",
            "stop-slop-stand guide",
        ),
    ];
    for (world, extra, fake, phase, next) in cases {
        let mut args = vec!["run"];
        args.extend(extra);
        args.push("--json");
        let ran = world.run(&fake, &args);
        assert_eq!(ran.exit, Exit::Refused, "{args:?}: {}", ran.err);
        let line = ran.line();
        assert_eq!(line["failure"]["phase"], phase, "{line}");
        assert!(line["run"].is_null(), "{line}");
        assert_eq!(ran.next(), [next], "{args:?}");
        assert!(!any_create(&fake), "{args:?}");
    }
}

#[test]
fn a_failed_head_is_1_without_a_report_and_names_inspect_or_the_recipe() {
    let world = World::new();
    // `api` dies in the head's first repeat: `db` keeps its failure snapshot.
    let died = fake(vec![on_tree(
        "api",
        world.tar("head"),
        exits(1, 2 * SECOND),
    )]);
    let ran = world.run(&died, &["run", "--json"]);
    assert_eq!(ran.exit, Exit::AppFailed, "{}", ran.err);
    let line = ran.line();
    let run = line["run"].as_str().unwrap();
    assert_eq!(
        line["next"],
        serde_json::json!([format!("stop-slop-stand inspect {run} head db -- <argv>")])
    );
    assert!(line["report"].is_null());
    assert!(!world.run_dir(run).join("report.json").exists());
    assert!(
        ran.err
            .starts_with("the head failed: workload died in api, repeat 1; logs "),
        "{}",
        ran.err
    );
    // Setup fails: nothing is kept, so the next step is the rules of the recipe.
    let world = World::new();
    let setup = fake(vec![on_tree(
        "prepare",
        world.tar("head"),
        exits(1, SECOND),
    )]);
    let ran = world.run(&setup, &["run"]);
    assert_eq!(ran.exit, Exit::AppFailed, "{}", ran.err);
    assert!(
        ran.out.ends_with("next: stop-slop-stand guide recipe\n"),
        "{}",
        ran.out
    );
    let line = world.run(&setup, &["run", "--json"]).line();
    assert_eq!(line["failure"]["phase"], "setup");
    assert_eq!(
        line["next"],
        serde_json::json!(["stop-slop-stand guide recipe"])
    );
}

#[test]
fn a_broken_backend_is_3_and_ctrl_c_is_130() {
    let world = World::new();
    let base = fake(Vec::new());
    let broken = Wrapped {
        fails_on: Some("prepare"),
        ..Wrapped::new(&base)
    };
    let ran = world.run(&broken, &["run", "--json"]);
    assert_eq!(ran.exit, Exit::Broken, "{}", ran.err);
    assert_eq!(ran.line()["failure"]["phase"], "setup");
    assert!(ran.line()["run"].is_string());
    assert_eq!(ran.next(), ["stop-slop-stand guide"]);
    // A result that is not a plain file: `report` refuses to read it, and the run is broken.
    let odd = fake(vec![rule(
        "load",
        Script {
            writes: vec![("/stand/out/odd.jsonl/inner.jsonl".into(), b"{}\n".to_vec())],
            ..exits(0, 10 * SECOND)
        },
    )]);
    let ran = World::new().run(&odd, &["run", "--json"]);
    assert_eq!(ran.exit, Exit::Broken, "{}", ran.err);
    assert_eq!(ran.line()["failure"]["phase"], "report");
    assert!(ran.err.contains("odd.jsonl"), "{}", ran.err);
    let cancel = Cancel::new();
    let base = fake(Vec::new());
    let hooked = Wrapped {
        cancels_on: Some(("api", &cancel)),
        ..Wrapped::new(&base)
    };
    let ran = world.run_on(&hooked, &["run", "--json"], &roomy, &cancel);
    assert_eq!(ran.exit, Exit::Cancelled, "{}", ran.err);
    assert_eq!(ran.line()["failure"]["cause"], "cancelled");
    assert!(ran.next().is_empty(), "{}", ran.err);
    assert!(
        base.leftovers().unwrap().sandboxes.is_empty(),
        "every VM destroyed"
    );
    let run = ran.line()["run"].as_str().unwrap().to_owned();
    assert!(world.run_dir(&run).is_dir());
    assert!(!world.tar("head").exists());
}

#[test]
fn a_bad_stand_toml_is_2_with_its_file_and_key() {
    let cases = [
        ("host_share = 90\n", "host_share"),
        ("max_tolerance = 2000\n", "max_tolerance"),
        ("x = [\n", "toml"),
    ];
    for (text, key) in cases {
        let world = World::new();
        fs::write(&world.paths.config, text).unwrap();
        let fake = fake(Vec::new());
        let ran = world.run(&fake, &["run", "--json"]);
        assert_eq!(ran.exit, Exit::Refused, "{text}");
        assert_eq!(ran.line()["failure"]["phase"], "config");
        let said = format!("{}: {key}: ", world.paths.config);
        assert!(ran.err.contains(&said), "{said} in {}", ran.err);
        assert!(!any_create(&fake));
    }
}

#[test]
fn a_second_run_pools_with_the_first_and_a_changed_stand_file_is_recipe_changed() {
    let world = World::new();
    let fake = fake(Vec::new());
    let first = world.run(&fake, &["run", "--json"]).line();
    let second = world.run(&fake, &["run", "--json"]).line();
    assert_eq!(second["attempts"], 2);
    let report = world.report(&second);
    assert_eq!(report["attempts"], 2);
    // The head did not change: the second run takes its setup from the cache.
    assert_eq!(report["head"]["setup"]["cached"], first["run"]);
    assert!(
        report["previousRun"].is_null(),
        "the pool is not the previous run"
    );
    world.write_recipe("compose.yaml", &format!("{RECIPE}# one more line\n"));
    let third = world.run(&fake, &["run", "--json"]).line();
    let report = world.report(&third);
    assert_eq!(report["attempts"], 1);
    assert_eq!(report["previousRun"], second["run"]);
    let flags = report["flags"].to_string();
    assert!(flags.contains("recipe-changed"), "{flags}");
}

#[test]
fn the_start_sweeps_leftovers_but_keeps_kept_and_cached_snapshots_and_the_ttl_forgets() {
    let world = World::new();
    let fake = fake(Vec::new());
    assert_eq!(world.run(&fake, &["run"]).exit, Exit::Passed);
    let session = world.sessions().remove(0);
    let (kept, cached) = (kept_of(&session), cached_of(&session));
    assert!(!kept.is_empty() && !cached.is_empty());
    assert_eq!(set(&snapshots_left(&fake)), &set(&kept) | &set(&cached));
    // A crashed run's VM and snapshot, which no run or slot names, and its tar.
    let stray = fake.create(&[db_launch(&fake)]).unwrap();
    let stray_snapshot = fake.snapshot(&stray[0]).unwrap();
    fs::create_dir_all(world.paths.root.join("trees")).unwrap();
    fs::write(world.tar("head"), b"partial").unwrap();
    // A dry run counts them and drops nothing (orchestrator, review of T-14).
    let mark = fake.journal().len();
    let dry = world.run(&fake, &["run", "--dry-run"]);
    let said = "\nwould destroy 1 VM that earlier runs left\n\
                would drop 1 snapshot that no kept run or cache entry names\n";
    assert!(dry.out.contains(said), "{}", dry.out);
    let swept = |call: &Call| matches!(call, Call::Destroy(_) | Call::DropSnapshot(_));
    assert!(!fake.journal()[mark..].iter().any(swept));
    let mark = fake.journal().len();
    assert_eq!(world.run(&fake, &["run"]).exit, Exit::Passed);
    let swept = before_first_create(&fake, mark);
    assert!(
        swept
            .iter()
            .any(|call| matches!(call, Call::Destroy(id) if *id == stray[0]))
    );
    assert_eq!(
        dropped(&swept),
        [stray_snapshot],
        "only the stray goes at the start"
    );
    // The second run is kept now, so the first's snapshots go; the cache entry stays in its slot.
    let left = set(&snapshots_left(&fake));
    assert!(
        set(&kept).is_disjoint(&left),
        "the first kept run's snapshots go"
    );
    assert!(set(&cached).is_subset(&left), "the cache entry stays");
    // The TTL: a session not used for three days is forgotten and its snapshots go; a run of
    // another session of the same stand folder sweeps it.
    age_sessions(&world, Duration::from_secs(4 * 24 * 3600));
    let other = world.worktree("other");
    let dry = world.run_at(&other, &fake, &["run", "--dry-run"], &roomy, &Cancel::new());
    let said = "\nwould forget the runs and cache of 1 expired session\n";
    assert!(dry.out.contains(said), "{}", dry.out);
    assert!(session.exists(), "the dry run forgets nothing");
    let ran = world.run_at(&other, &fake, &["run"], &roomy, &Cancel::new());
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    let now = set(&snapshots_left(&fake));
    assert!(left.is_disjoint(&now), "the expired session's snapshots go");
    assert!(!session.exists(), "its folder goes");
    assert_eq!(world.sessions().len(), 1);
    let prune = fake
        .journal()
        .into_iter()
        .rev()
        .find_map(|call| match call {
            Call::PruneImages(keep) => Some(keep),
            _ => None,
        });
    assert!(
        prune.is_some_and(|keep| keep.contains(&ImageRef::new("postgres:17"))),
        "the live session keeps its images"
    );
}

/// A `Launch` of `db` as the first run made it, for a VM a crashed run would leave.
fn db_launch(fake: &Fake) -> Launch {
    fake.journal()
        .into_iter()
        .find_map(|call| match call {
            Call::Create { batch, .. } => batch
                .into_iter()
                .find(|launch| launch.service == "db" && launch.seed.is_none()),
            _ => None,
        })
        .expect("the first run created db")
}

/// `usedAt` of every session of the world, moved back (store's `session.json`, a law of the blueprint).
fn age_sessions(world: &World, by: Duration) {
    for session in world.sessions() {
        let path = session.join("session.json");
        let mut state = read_json(&path);
        let old = SystemTime::now() - by;
        state["usedAt"] = Value::String(humantime::format_rfc3339(old).to_string());
        fs::write(&path, state.to_string()).unwrap();
    }
}

#[test]
fn a_low_disk_drops_the_cache_and_kept_run_of_others_then_its_own_cache_then_its_kept_run() {
    let world = World::new();
    let fake = fake(Vec::new());
    let other = world.worktree("other");
    let ran = world.run_at(&other, &fake, &["run"], &roomy, &Cancel::new());
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    let others = set(&snapshots_left(&fake));
    let theirs = world.sessions();
    assert_eq!(world.run(&fake, &["run"]).exit, Exit::Passed);
    let own = world
        .sessions()
        .into_iter()
        .find(|session| !theirs.contains(session))
        .unwrap();
    let (own_kept, own_cached) = (set(&kept_of(&own)), set(&cached_of(&own)));
    assert!(!own_kept.is_empty() && !own_cached.is_empty());
    // A dry run on a low disk drops nothing and says what a run would drop, in turn; the kept run
    // still serves `inspect` after it (orchestrator, review of T-14).
    let low = |dir: &Utf8Path| {
        let mut host = roomy(dir)?;
        host.free_disk = 10 * GIB;
        Ok(host)
    };
    let kept_run = |session: &Utf8Path| {
        let state = read_json(&session.join("session.json"));
        state["kept"].as_str().unwrap().to_owned()
    };
    let (own_run, their_run) = (kept_run(&own), kept_run(&theirs[0]));
    let dry_mark = fake.journal().len();
    let dry = world.run_on(&fake, &["run", "--dry-run"], &low, &Cancel::new());
    assert_eq!(dry.exit, Exit::Passed, "{}", dry.err);
    let said = [
        "disk: 10.0 GiB free on the volume of the stand's folder, under the 20.0 GiB it keeps \
         free for snapshots: a run drops in turn until there is room"
            .to_owned(),
        format!("would drop 1 cache entry and 1 kept run ({their_run}) of other sessions"),
        "would drop 1 cache entry of this session".to_owned(),
        format!("would drop 1 kept run ({own_run}) of this session"),
    ];
    let lines: Vec<&str> = dry.out.lines().skip(1).take(4).collect();
    assert_eq!(lines, said, "{}", dry.out);
    let touched = |call: &Call| {
        matches!(
            call,
            Call::DropSnapshot(_) | Call::Destroy(_) | Call::Create { .. } | Call::Pull(_)
        )
    };
    assert!(!fake.journal()[dry_mark..].iter().any(touched));
    assert_eq!(set(&kept_of(&own)), own_kept);
    assert_eq!(set(&cached_of(&own)), own_cached);
    assert!(own_kept.is_subset(&set(&snapshots_left(&fake))));
    let looked = world.run(&fake, &["inspect", &own_run, "head", "db", "--", "true"]);
    assert_eq!(looked.exit, Exit::Passed, "{}", looked.err);
    // A run the CPUs refuse keeps all of it on that disk: only the disk is worth what it keeps.
    let small = |dir: &Utf8Path| {
        let mut host = low(dir)?;
        host.cpus = 2;
        Ok(host)
    };
    let mark = fake.journal().len();
    let ran = world.run_on(&fake, &["run", "--json"], &small, &Cancel::new());
    assert_eq!(ran.exit, Exit::Refused, "{}", ran.err);
    let cause = ran.line()["failure"]["cause"].to_string();
    assert!(cause.contains("cpu: requested"), "{cause}");
    assert!(dropped(&fake.journal()[mark..]).is_empty());
    // Low until the third read: the other session's snapshots and this one's cache go, its kept
    // run stays.
    let reads = Cell::new(0);
    let host = |dir: &Utf8Path| {
        reads.set(reads.get() + 1);
        let mut host = roomy(dir)?;
        if reads.get() < 3 {
            host.free_disk = 10 * GIB;
        }
        Ok(host)
    };
    let mark = fake.journal().len();
    let ran = world.run_on(&fake, &["run"], &host, &Cancel::new());
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    assert_eq!(reads.get(), 3);
    let dropped = dropped(&before_first_create(&fake, mark));
    let (first, then) = dropped.split_at(others.len().min(dropped.len()));
    assert_eq!(set(first), others, "the others' go first");
    assert_eq!(
        set(then),
        own_cached,
        "then this session's cache, and not its kept run"
    );
    // Its setup came from no cache: the slot was gone.
    let own_runs = folders(&own.join("runs"));
    let last = own_runs.last().unwrap();
    let report = read_json(&last.join("report.json"));
    assert!(report["head"]["setup"]["cached"].is_null());
    // The disk low to the end: the kept run goes too, and the budget refuses.
    let low = |dir: &Utf8Path| {
        let mut host = roomy(dir)?;
        host.free_disk = 10 * GIB;
        Ok(host)
    };
    let kept = set(&kept_of(&own));
    let ran = world.run_on(&fake, &["run", "--json"], &low, &Cancel::new());
    assert_eq!(ran.exit, Exit::Refused, "{}", ran.err);
    assert_eq!(ran.line()["failure"]["phase"], "budget");
    assert!(
        kept.is_disjoint(&set(&snapshots_left(&fake))),
        "its kept run went too"
    );
}

#[test]
fn a_pooled_run_takes_coverage_from_the_pool_and_runs_it_again_when_unknown() {
    // A coverage chunk of `api`: the coverage is known, and the second run takes it.
    let world = World::new();
    let chunks = fake(vec![profiled_api()]);
    let first = world.run(&chunks, &["run", "--json"]).line();
    assert!(
        world.report(&first)["coverage"]["from"].is_null(),
        "{}",
        world.report(&first)["coverage"]
    );
    let before = creates(&chunks, "api");
    let second = world.run(&chunks, &["run", "--json"]).line();
    // Two measured repeats of one replica, and no coverage repeat.
    assert_eq!(creates(&chunks, "api") - before, 2);
    let report = world.report(&second);
    assert_eq!(report["coverage"]["from"], first["run"]);
    let run = second["run"].as_str().unwrap();
    assert!(!world.run_dir(run).join("coverage").exists());
    // No chunk: the coverage stays unknown, and every run repeats it.
    let world = World::new();
    let fake = fake(Vec::new());
    let first = world.run(&fake, &["run", "--json"]).line();
    assert!(world.report(&first)["coverage"].is_null());
    let before = creates(&fake, "api");
    let second = world.run(&fake, &["run", "--json"]).line();
    assert_eq!(creates(&fake, "api") - before, 3);
    let run = second["run"].as_str().unwrap();
    assert!(world.run_dir(run).join("coverage").is_dir());
}

#[test]
fn a_pooled_run_takes_the_fault_and_the_second_pass_and_runs_them_again_when_unknown() {
    let recipe = RECIPE.replace("    x-stand: { profile: node }\n", "").replace(
        "  repeat: 2\n",
        "  repeat: 2\n  second_pass: true\n  fault: { target: \"db:5432\", mode: down }\n  \
         expect:\n    unchanged: [{ file: probe.orders.jsonl, reason: the workload only reads orders }]\n",
    );
    let world = World::with(&recipe);
    let fake = fake(Vec::new());
    let first = world.run(&fake, &["run", "--json"]);
    assert_eq!(first.exit, Exit::Passed, "{}", first.err);
    let first = first.line();
    let report = world.report(&first);
    assert!(
        report["fault"]["from"].is_null() && report["secondPass"]["from"].is_null(),
        "{report}"
    );
    assert!(faults(&fake) > 0);
    let mark = faults(&fake);
    let second = world.run(&fake, &["run", "--json"]).line();
    assert_eq!(faults(&fake), mark, "no fault repeat");
    let report = world.report(&second);
    assert_eq!(report["fault"]["from"], first["run"]);
    assert_eq!(report["secondPass"]["from"], first["run"]);
    let dir = world.run_dir(second["run"].as_str().unwrap());
    assert!(!dir.join("pass2-head").exists() && !dir.join("fault-1-head").exists());
    // The last report of the pool knows no category (no head repeat reached the fault) and has no
    // second pass: the next run makes both again.
    let path = dir.join("report.json");
    let mut last = read_json(&path);
    for category in last["fault"]["categories"]["head"].as_array_mut().unwrap() {
        for value in category["values"].as_array_mut().unwrap() {
            *value = Value::String("unknown".into());
        }
    }
    last["secondPass"] = Value::Null;
    fs::write(&path, last.to_string()).unwrap();
    let third = world.run(&fake, &["run", "--json"]).line();
    assert!(faults(&fake) > mark, "the fault repeats again");
    let report = world.report(&third);
    assert!(
        report["fault"]["from"].is_null() && report["secondPass"]["from"].is_null(),
        "{report}"
    );
    assert!(
        world
            .run_dir(third["run"].as_str().unwrap())
            .join("pass2-head")
            .is_dir()
    );
}

#[test]
fn a_failed_repeat_keeps_the_slot_and_a_breakage_or_ctrl_c_drops_what_the_run_built() {
    let world = World::new();
    // The head's setup passes, then `api` dies: exit 1, and the entry stays for the next run.
    let died = fake(vec![on_tree(
        "api",
        world.tar("head"),
        Script {
            exit: Some((1, 5 * SECOND)),
            ..healthy()
        },
    )]);
    assert_eq!(world.run(&died, &["run"]).exit, Exit::AppFailed);
    let prepares = creates(&died, "prepare");
    assert_eq!(world.run(&died, &["run"]).exit, Exit::AppFailed);
    assert_eq!(
        creates(&died, "prepare"),
        prepares,
        "the head's setup comes from the entry"
    );
    // A breakage after the setup of a new head tree: the slot stays as it was, and what this run
    // built goes.
    let world = World::new();
    let base = fake(Vec::new());
    assert_eq!(world.run(&base, &["run"]).exit, Exit::Passed);
    let (entries_then, left_then) = (entries(&world), set(&snapshots_left(&base)));
    world.repo.write("main.js", "console.log('v2')\n");
    let broken = Wrapped {
        fails_on: Some("api"),
        ..Wrapped::new(&base)
    };
    let prepares = creates(&base, "prepare");
    assert_eq!(world.run(&broken, &["run"]).exit, Exit::Broken);
    assert_eq!(creates(&base, "prepare") - prepares, 1, "its setup ran");
    assert_eq!(entries(&world), entries_then);
    assert_eq!(set(&snapshots_left(&base)), left_then);
    // It kept nothing, so the kept run is still the first.
    let session = world.sessions().remove(0);
    let first = folders(&session.join("runs"))[0].clone();
    let state = read_json(&session.join("session.json"));
    assert_eq!(state["kept"], first.file_name().unwrap());
    // Ctrl+C after setup: nothing of the cancelled run stays.
    let world = World::new();
    let base = fake(Vec::new());
    let cancel = Cancel::new();
    let hooked = Wrapped {
        cancels_on: Some(("api", &cancel)),
        ..Wrapped::new(&base)
    };
    assert_eq!(
        world.run_on(&hooked, &["run"], &roomy, &cancel).exit,
        Exit::Cancelled
    );
    assert!(creates(&base, "prepare") > 0, "its setup ran");
    assert!(
        entries(&world).is_empty(),
        "no entry of the cancelled run stays"
    );
    assert!(snapshots_left(&base).is_empty(), "nor its snapshots");
}

/// Each step of settling on its own (batch review 6): a cache that fails to settle costs neither
/// the kept run nor the tar files, and the head's failure still shows.
#[test]
fn a_cache_that_fails_to_settle_still_keeps_the_run_and_clears_the_trees() {
    let world = World::new();
    let died = fake(vec![on_tree(
        "api",
        world.tar("head"),
        exits(1, 2 * SECOND),
    )]);
    assert_eq!(world.run(&died, &["run"]).exit, Exit::AppFailed);
    // A new head tree: its entry replaces the first one, whose snapshots will not drop.
    let session = world.sessions().remove(0);
    let first = cached_of(&session);
    world.repo.write("main.js", "console.log('v2')\n");
    let broken = Wrapped {
        undroppable: &first,
        ..Wrapped::new(&died)
    };
    let ran = world.run(&broken, &["run", "--json"]);
    assert_eq!(ran.exit, Exit::Broken, "{}", ran.err);
    let line = ran.line();
    assert_eq!(line["failure"]["phase"], "report");
    assert!(
        ran.err.contains("the head failed: workload died in api"),
        "{}",
        ran.err
    );
    let state = read_json(&session.join("session.json"));
    assert_eq!(state["kept"], line["run"]);
    assert!(!world.tar("head").exists());
    let args = [
        "inspect",
        line["run"].as_str().unwrap(),
        "head",
        "db",
        "--",
        "true",
    ];
    let looked = world.run(&died, &args);
    assert_eq!(looked.exit, Exit::Passed, "{}", looked.err);
}

/// The before slot lets go of an entry the head takes in the same run: it keeps its snapshots
/// (batch review 6).
#[test]
fn a_run_with_the_two_trees_swapped_takes_both_setups_and_keeps_both_entries() {
    let world = World::new();
    let fake = fake(Vec::new());
    world.run(&fake, &["snapshot"]);
    world.repo.write("main.js", "console.log('v2')\n");
    let first = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    // The snapshot takes the new tree, and the work tree goes back to the old one.
    world.run(&fake, &["snapshot"]);
    world.repo.write("main.js", "console.log('v1')\n");
    let prepares = creates(&fake, "prepare");
    let swapped = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    assert_eq!(creates(&fake, "prepare"), prepares, "both from the cache");
    let report = world.report(&swapped);
    assert_eq!(report["before"]["setup"]["cached"], first["run"]);
    assert_eq!(report["head"]["setup"]["cached"], first["run"]);
    let session = world.sessions().remove(0);
    assert!(set(&cached_of(&session)).is_subset(&set(&snapshots_left(&fake))));
    let again = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    assert_eq!(
        creates(&fake, "prepare"),
        prepares,
        "both from the cache again"
    );
    assert_eq!(
        world.report(&again)["head"]["setup"]["cached"],
        first["run"]
    );
}

#[test]
fn a_stand_run_outside_git_is_2() {
    let world = World::new();
    let outside = tempfile::tempdir().unwrap();
    let cwd = Utf8Path::from_path(outside.path()).unwrap();
    let fake = fake(Vec::new());
    let ran = world.run_at(cwd, &fake, &["run", "--json"], &roomy, &Cancel::new());
    assert_eq!(ran.exit, Exit::Refused);
    assert_eq!(ran.line()["failure"]["phase"], "source");
    assert!(
        ran.err.contains("not inside a git work tree"),
        "{}",
        ran.err
    );
    assert!(!any_create(&fake));
}

#[test]
fn snapshot_prints_its_sha_and_then_the_one_it_replaced() {
    let world = World::new();
    let fake = fake(Vec::new());
    let first = world.run(&fake, &["snapshot"]);
    assert_eq!(first.exit, Exit::Passed, "{}", first.err);
    let sha = first.out.trim().to_owned();
    assert_eq!(sha.len(), 40, "{sha}");
    let index = world.repo.git(&["status", "--porcelain"]);
    world.repo.write("main.js", "console.log('v2')\n");
    let second = world.run(&fake, &["snapshot"]);
    assert!(
        second.out.ends_with(&format!("\nreplaced {sha}\n")),
        "{}",
        second.out
    );
    world.repo.write("main.js", "console.log('v1')\n");
    assert_eq!(
        world.repo.git(&["status", "--porcelain"]),
        index,
        "the index stays"
    );
    assert!(!any_create(&fake));
}

#[test]
fn clean_drops_snapshots_before_images_and_keeps_the_journal() {
    let world = World::new();
    let fake = fake(Vec::new());
    world.run(&fake, &["snapshot"]);
    world.repo.write("main.js", "console.log('v2')\n");
    let sealed = world.run(&fake, &["run", "--before", "session"]);
    assert_eq!(sealed.exit, Exit::Passed, "{}", sealed.err);
    assert!(!snapshots_left(&fake).is_empty());
    let ran = world.run(&fake, &["clean"]);
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    let journal = fake.journal();
    let last_drop = journal
        .iter()
        .rposition(|call| matches!(call, Call::DropSnapshot(_)))
        .unwrap();
    let prune = journal
        .iter()
        .rposition(|call| matches!(call, Call::PruneImages(keep) if keep.is_empty()))
        .unwrap();
    assert!(last_drop < prune);
    assert!(snapshots_left(&fake).is_empty());
    assert!(!world.paths.root.join("sessions").exists());
    let journals: Vec<String> = fs::read_dir(world.paths.root.join("journal"))
        .unwrap()
        .flatten()
        .map(|entry| fs::read_to_string(entry.path()).unwrap())
        .collect();
    assert!(
        journals.len() == 1 && journals[0].contains("cleanedAt"),
        "{journals:?}"
    );
    assert_eq!(
        world.repo.git(&["for-each-ref", "refs/stop-slop/stand"]),
        "",
        "the snapshot ref goes"
    );
    // The next seal shows the clean in its journal.
    world.run(&fake, &["snapshot"]);
    world.repo.write("main.js", "console.log('v3')\n");
    let line = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    let verbs: Vec<&str> = line["seal"]["journal"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            let mut keys = entry.as_object().unwrap().keys();
            keys.find(|key| key.ends_with("At")).unwrap().as_str()
        })
        .collect();
    assert_eq!(verbs, ["sealedAt", "cleanedAt", "sealedAt"], "{line}");
    // `--all` leaves the lock and nothing else of the folder (`stand.toml` lies outside it here).
    let mark = fake.journal().len();
    assert_eq!(world.run(&fake, &["clean", "--all"]).exit, Exit::Passed);
    assert!(fake.journal().len() == mark, "no call of the adapter");
    let left: Vec<String> = fs::read_dir(&world.paths.root)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["lock"]);
    assert_eq!(
        world.repo.git(&["for-each-ref", "refs/stop-slop/stand"]),
        ""
    );
}

/// The folder of the entry a cache slot of a session names (`cache.<slot>.digest` of
/// `session.json`, a law of the blueprint).
fn slot_entry(session: &Utf8Path, slot: &str) -> Utf8PathBuf {
    let state = read_json(&session.join("session.json"));
    let digest = state["cache"][slot]["digest"]
        .as_str()
        .unwrap_or_else(|| panic!("no {slot} slot: {state}"));
    session.join("cache").join(digest)
}

/// The run that `measured-before-seal` of a line names.
fn measured_before_seal(line: &Value) -> Option<&str> {
    let flags = line["flags"].as_array()?;
    let flag = flags
        .iter()
        .find(|flag| flag["kind"] == "measured-before-seal")?;
    flag["run"].as_str()
}

#[test]
fn before_session_right_after_the_snapshot_has_nothing_to_compare() {
    let world = World::new();
    let fake = fake(Vec::new());
    world.run(&fake, &["snapshot"]);
    let runs: [&[&str]; 2] = [
        &["run", "--before", "session", "--json"],
        &["run", "--before", "session", "--dry-run", "--json"],
    ];
    for args in runs {
        let ran = world.run(&fake, args);
        assert_eq!(ran.exit, Exit::Refused, "{args:?}: {}", ran.err);
        assert_eq!(ran.line()["failure"]["phase"], "source", "{args:?}");
        assert!(ran.err.contains("nothing to compare"), "{}", ran.err);
        assert_eq!(ran.next(), ["stop-slop-stand guide"]);
    }
    // The trees come first: a run that cannot compare seals, sweeps and pulls nothing.
    assert!(fake.journal().is_empty(), "{:?}", fake.journal());
    assert!(
        world
            .sessions()
            .iter()
            .all(|session| !session.join("seal").exists())
    );
}

#[test]
fn the_first_run_before_seals_and_a_changed_file_refuses_until_a_reseal() {
    let world = World::new();
    let fake = fake(Vec::new());
    world.run(&fake, &["snapshot"]);
    world.repo.write("main.js", "console.log('v2')\n");
    let first = world.run(&fake, &["run", "--before", "session", "--json"]);
    assert_eq!(first.exit, Exit::Passed, "{}", first.err);
    let journal = first.line()["seal"]["journal"].clone();
    assert!(
        journal.as_array().is_some_and(|entries| entries.len() == 1)
            && journal[0]["sealedAt"].is_string(),
        "{journal}"
    );
    // The run that sealed is not measured before its seal (orchestrator, T-13b): the next run is
    // the second comparison, and no run of this head came before the seal.
    let second = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    assert_eq!(second["comparisons"], 2, "{second}");
    assert_eq!(measured_before_seal(&second), None, "{second}");
    // An edit of the recipe folder refuses with the file and the command that reseals.
    world.write_recipe("load.js", "// load, edited\n");
    let changed = world.run(&fake, &["run", "--before", "session", "--json"]);
    assert_eq!(changed.exit, Exit::Refused, "{}", changed.err);
    let line = changed.line();
    assert_eq!(line["failure"]["phase"], "seal");
    let cause = line["failure"]["cause"].as_str().unwrap();
    assert!(cause.contains("load.js"), "{line}");
    assert!(changed.err.contains("load.js"), "{}", changed.err);
    let reseal = "stop-slop-stand run --before session --reseal --reason \"<text>\"";
    assert_eq!(line["next"], serde_json::json!([reseal]));
    assert_eq!(changed.next(), [reseal]);
    // Without --before the live recipe runs, with no seal in its line.
    let plain = world.run(&fake, &["run", "--json"]);
    assert_eq!(plain.exit, Exit::Passed, "{}", plain.err);
    let plain = plain.line();
    assert!(
        plain["seal"].is_null() && plain["comparisons"].is_null(),
        "{plain}"
    );
    let args = [
        "run",
        "--before",
        "session",
        "--reseal",
        "--reason",
        "a probe of orders",
        "--json",
    ];
    let resealed = world.run(&fake, &args);
    assert_eq!(resealed.exit, Exit::Passed, "{}", resealed.err);
    assert_eq!(resealed.line()["comparisons"], 1, "the count starts again");
    let next = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    let journal = next["seal"]["journal"].to_string();
    assert!(
        journal.contains("resealedAt") && journal.contains("a probe of orders"),
        "{journal}"
    );
    assert_eq!(next["comparisons"], 2, "{next}");
    // The last run of this head before the reseal is the plain one, not the run that resealed.
    assert_eq!(measured_before_seal(&next), plain["run"].as_str(), "{next}");
}

/// A run writes its seal after its pull (batch review 6): a first run the budget refuses leaves
/// none, so the recipe it fixes runs without a reseal.
#[test]
fn a_first_run_before_that_the_budget_refuses_seals_nothing() {
    let world = World::new();
    let fake = fake(Vec::new());
    world.run(&fake, &["snapshot"]);
    world.repo.write("main.js", "console.log('v2')\n");
    let small = |dir: &Utf8Path| {
        let mut host = roomy(dir)?;
        host.cpus = 2;
        Ok(host)
    };
    let args = ["run", "--before", "session", "--json"];
    let refused = world.run_on(&fake, &args, &small, &Cancel::new());
    assert_eq!(refused.exit, Exit::Refused, "{}", refused.err);
    assert_eq!(refused.line()["failure"]["phase"], "budget");
    let sealed = |world: &World| world.sessions().iter().any(|s| s.join("seal").exists());
    assert!(!sealed(&world));
    world.write_recipe("load.js", "// load, smaller\n");
    let ran = world.run(&fake, &args);
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    let journal = ran.line()["seal"]["journal"].clone();
    assert_eq!(journal.as_array().map(Vec::len), Some(1), "{journal}");
    assert!(sealed(&world));
}

#[test]
fn dry_run_creates_nothing_pulls_nothing_never_seals_and_names_the_cache() {
    let world = World::new();
    let fake = fake(Vec::new());
    world.run(&fake, &["snapshot"]);
    world.repo.write("main.js", "console.log('v2')\n");
    let ran = world.run(&fake, &["run", "--before", "session", "--dry-run"]);
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    let first = ran.out.lines().next().unwrap();
    assert!(
        first.starts_with("dry run: ") && first.contains("compose.yaml (live folder), recipe "),
        "{}",
        ran.out
    );
    assert!(!first.contains('\\'), "{first}");
    assert!(
        ran.out.lines().nth(1).unwrap().starts_with("budget: "),
        "{}",
        ran.out
    );
    assert!(
        ran.out
            .contains("\nimage postgres:17 cached fake:postgres:17\n")
            && ran.out.contains("\nimage node:22 cached fake:node:22\n"),
        "{}",
        ran.out
    );
    assert!(
        ran.out
            .ends_with("\nsetup head built\nsetup before built\n"),
        "{}",
        ran.out
    );
    let json = world.run(
        &fake,
        &["run", "--before", "session", "--dry-run", "--json"],
    );
    assert_eq!(json.exit, Exit::Passed, "{}", json.err);
    let line = json.line();
    assert!(
        line["run"].is_null() && line["report"].is_null() && line["failure"].is_null(),
        "{line}"
    );
    let made = |call: &Call| {
        matches!(
            call,
            Call::Create { .. } | Call::Pull(_) | Call::DropSnapshot(_)
        )
    };
    assert!(!fake.journal().iter().any(made), "{:?}", fake.journal());
    assert!(
        world.sessions().iter().all(|session| {
            !session.join("seal").exists() && folders(&session.join("runs")).is_empty()
        }),
        "no seal and no run folder"
    );
    // After a run, the dry run reads the seal and names the run whose setups it would take.
    let line = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    let run = line["run"].as_str().unwrap();
    let mark = fake.journal().len();
    let again = world.run(&fake, &["run", "--before", "session", "--dry-run"]);
    assert!(
        again.out.contains("compose.yaml (sealed copy of "),
        "{}",
        again.out
    );
    let both = format!("\nsetup head cached {run}\nsetup before cached {run}\n");
    assert!(again.out.ends_with(&both), "{}", again.out);
    assert!(!fake.journal()[mark..].iter().any(made), "it drops nothing");
    // An image gone from the home would download, so no setup can come from the cache.
    let evicted = Wrapped {
        evicted: &["node:22"],
        ..Wrapped::new(&fake)
    };
    let gone = world.run(&evicted, &["run", "--before", "session", "--dry-run"]);
    assert!(
        gone.out.contains("\nimage node:22 to download\n")
            && gone
                .out
                .ends_with("\nsetup head built\nsetup before built\n"),
        "{}",
        gone.out
    );
    // The refusals of `run` are its refusals: an edit of the sealed recipe, a stand too big.
    world.write_recipe("load.js", "// load, edited\n");
    let changed = world.run(
        &fake,
        &["run", "--before", "session", "--dry-run", "--json"],
    );
    assert_eq!(changed.exit, Exit::Refused, "{}", changed.err);
    assert_eq!(changed.line()["failure"]["phase"], "seal");
    let live = world.run(&fake, &["run", "--dry-run"]);
    assert_eq!(live.exit, Exit::Passed, "{}", live.err);
    assert!(
        live.out.contains("compose.yaml (live folder)")
            && live.out.ends_with("\nsetup head built\n"),
        "another recipe, another key: {}",
        live.out
    );
    let small = |dir: &Utf8Path| {
        let mut host = roomy(dir)?;
        host.cpus = 2;
        Ok(host)
    };
    let ran = world.run_on(
        &fake,
        &["run", "--dry-run", "--json"],
        &small,
        &Cancel::new(),
    );
    assert_eq!(ran.exit, Exit::Refused, "{}", ran.err);
    assert_eq!(ran.line()["failure"]["phase"], "budget");
    assert!(!fake.journal()[mark..].iter().any(made));
}

#[test]
fn a_second_run_before_session_takes_the_before_setup_from_the_cache() {
    let world = World::new();
    let fake = fake(Vec::new());
    world.run(&fake, &["snapshot"]);
    world.repo.write("main.js", "console.log('v2')\n");
    let first = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    let report = world.report(&first);
    assert!(
        report["before"]["setup"]["cached"].is_null()
            && report["head"]["setup"]["cached"].is_null()
    );
    // No edit: both setups come from the entries of the first run.
    let prepares = creates(&fake, "prepare");
    let second = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    assert_eq!(creates(&fake, "prepare"), prepares, "no setup runs");
    let report = world.report(&second);
    assert_eq!(report["before"]["setup"]["cached"], first["run"]);
    assert_eq!(report["head"]["setup"]["cached"], first["run"]);
    // Another head tree: only the head sets up again.
    world.repo.write("main.js", "console.log('v3')\n");
    let prepares = creates(&fake, "prepare");
    let third = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    assert_eq!(creates(&fake, "prepare") - prepares, 1);
    let report = world.report(&third);
    assert_eq!(report["before"]["setup"]["cached"], first["run"]);
    assert!(report["head"]["setup"]["cached"].is_null());
    // Another digest of an image misses for both versions.
    let moved = Wrapped {
        digests: &[("postgres:17", "sha256:moved")],
        ..Wrapped::new(&fake)
    };
    let prepares = creates(&fake, "prepare");
    let fourth = world
        .run(&moved, &["run", "--before", "session", "--json"])
        .line();
    assert_eq!(creates(&fake, "prepare") - prepares, 2);
    assert!(world.report(&fourth)["before"]["setup"]["cached"].is_null());
    // A reseal puts another recipe digest in the key: a miss again.
    world.write_recipe("load.js", "// load, edited\n");
    let prepares = creates(&fake, "prepare");
    let args = [
        "run",
        "--before",
        "session",
        "--reseal",
        "--reason",
        "edited load",
        "--json",
    ];
    let fifth = world.run(&moved, &args);
    assert_eq!(fifth.exit, Exit::Passed, "{}", fifth.err);
    assert_eq!(creates(&fake, "prepare") - prepares, 2);
}

#[test]
fn the_slots_follow_the_last_run_and_a_head_entry_serves_the_next_before() {
    let world = World::new();
    let fake = fake(Vec::new());
    world.run(&fake, &["snapshot"]);
    // Without --before the head's entry fills the head slot; its tree is the snapshot's.
    let alone = world.run(&fake, &["run", "--json"]).line();
    world.repo.write("main.js", "console.log('v2')\n");
    let prepares = creates(&fake, "prepare");
    let compared = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    // "Before" hits the entry the head built a run ago; only the new head sets up.
    assert_eq!(creates(&fake, "prepare") - prepares, 1);
    assert_eq!(
        world.report(&compared)["before"]["setup"]["cached"],
        alone["run"]
    );
    let session = world.sessions().remove(0);
    let head_entry = slot_entry(&session, "head");
    let head_ids = run::cached_ids(&head_entry).unwrap().unwrap();
    // A run without --before changes only the head slot: the entry it replaces goes, snapshots
    // and folder, and "before" still hits after it.
    world.repo.write("main.js", "console.log('v3')\n");
    let plain = world.run(&fake, &["run", "--json"]).line();
    assert!(!head_entry.exists());
    assert!(set(&head_ids).is_disjoint(&set(&snapshots_left(&fake))));
    let prepares = creates(&fake, "prepare");
    let next = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    assert_eq!(creates(&fake, "prepare"), prepares, "both from the cache");
    let report = world.report(&next);
    assert_eq!(report["before"]["setup"]["cached"], alone["run"]);
    assert_eq!(report["head"]["setup"]["cached"], plain["run"]);
    // An entry with a snapshot gone from the home is a miss: only "before" sets up again.
    let before_ids = run::cached_ids(&slot_entry(&session, "before"))
        .unwrap()
        .unwrap();
    fake.drop_snapshot(&before_ids[0]).unwrap();
    let dry = world.run(&fake, &["run", "--before", "session", "--dry-run"]);
    let plain_run = plain["run"].as_str().unwrap();
    let said = format!("\nsetup head cached {plain_run}\nsetup before built\n");
    assert!(dry.out.ends_with(&said), "{}", dry.out);
    let prepares = creates(&fake, "prepare");
    let missed = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    assert_eq!(creates(&fake, "prepare") - prepares, 1);
    let report = world.report(&missed);
    assert!(report["before"]["setup"]["cached"].is_null());
    assert_eq!(report["head"]["setup"]["cached"], plain["run"]);
}

/// `db` whose first `inspect` output, `1.out`, is 50 rows with the code 3.
fn inspected_db() -> Rule {
    let rows: String = (1..=50).map(|n| format!("row {n}\n")).collect();
    rule(
        "db",
        Script {
            volume_bytes: 4096,
            probes: vec![
                (
                    "probe.orders.jsonl".into(),
                    ExecOutcome::Exited(0),
                    b"{\"id\":1}\n".to_vec(),
                ),
                ("1.out".into(), ExecOutcome::Exited(3), rows.into_bytes()),
            ],
            ..healthy()
        },
    )
}

/// `<run>/inspect/<n>.out` as `inspect` prints it.
fn looked(world: &World, run: &str, n: u32) -> String {
    let out = world.run_dir(run).join("inspect").join(format!("{n}.out"));
    out.as_str().replace('\\', "/")
}

#[test]
fn inspect_prints_the_code_forty_lines_and_the_path_and_refuses_what_is_not_kept() {
    let world = World::new();
    let fake = fake(vec![inspected_db()]);
    let line = world.run(&fake, &["run", "--json"]).line();
    let run = line["run"].as_str().unwrap();
    let snapshots = set(&snapshots_left(&fake));
    let args = ["inspect", run, "head", "db", "--", "psql", "-c", "select 1"];
    let ran = world.run(&fake, &args);
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    let lines: Vec<&str> = ran.out.lines().collect();
    assert_eq!(lines.len(), 1 + 40 + 1, "{}", ran.out);
    assert_eq!(
        (lines[0], lines[1], lines[40]),
        ("exit 3", "row 1", "row 40")
    );
    assert_eq!(lines[41], format!("full output {}", looked(&world, run, 1)));
    let argv = fake
        .journal()
        .into_iter()
        .rev()
        .find_map(|call| match call {
            Call::Exec { argv, .. } => Some(argv),
            _ => None,
        });
    assert_eq!(argv.unwrap(), ["psql", "-c", "select 1"]);
    // The copy is gone and the snapshots are as they were.
    assert!(fake.leftovers().unwrap().sandboxes.is_empty());
    assert_eq!(set(&snapshots_left(&fake)), snapshots);
    let again = world.run(&fake, &["inspect", run, "head", "db", "--", "true"]);
    let said = format!("exit 0\nfull output {}\n", looked(&world, run, 2));
    assert_eq!(again.out, said);
    // One line of a megabyte: 64 KiB of it are read and printed.
    let long = |stdout: &Utf8Path| {
        fs::write(stdout, format!("{}\nrow 2\n", "x".repeat(1 << 20))).unwrap();
    };
    let swapped = Wrapped {
        after_exec: Some(&long),
        ..Wrapped::new(&fake)
    };
    let ran = world.run(&swapped, &["inspect", run, "head", "db", "--", "true"]);
    let lines: Vec<&str> = ran.out.lines().collect();
    assert_eq!(lines.len(), 3, "{}", ran.err);
    assert_eq!(lines[1], "x".repeat(64 << 10));
    assert_eq!(lines[2], format!("full output {}", looked(&world, run, 3)));
    // What `argv` leaves in place of its output is not a regular file: nothing of it is printed.
    let folder = |stdout: &Utf8Path| {
        fs::remove_file(stdout).unwrap();
        fs::create_dir(stdout).unwrap();
    };
    let swapped = Wrapped {
        after_exec: Some(&folder),
        ..Wrapped::new(&fake)
    };
    let ran = world.run(&swapped, &["inspect", run, "head", "db", "--", "true"]);
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    let not_regular = |n| {
        let path = looked(&world, run, n);
        format!("exit 0\nthe output {path} is not a regular file; nothing of it is printed\n")
    };
    assert_eq!(ran.out, not_regular(4));
    // Windows makes a link only in developer mode or for an administrator, so Linux CI runs this.
    #[cfg(unix)]
    {
        let secret = world.paths.root.join("secret");
        fs::write(&secret, "a file of the host\n").unwrap();
        let link = |stdout: &Utf8Path| {
            fs::remove_file(stdout).unwrap();
            std::os::unix::fs::symlink(&secret, stdout).unwrap();
        };
        let linked = Wrapped {
            after_exec: Some(&link),
            ..Wrapped::new(&fake)
        };
        let ran = world.run(&linked, &["inspect", run, "head", "db", "--", "true"]);
        assert_eq!(ran.out, not_regular(5));
    }
    // Another run, a side or a service the kept run has no snapshot of: 2, and no VM.
    let mark = fake.journal().len();
    let refusals: [(&[&str], String); 3] = [
        (
            &["inspect", "20200101-000000", "head", "db", "--", "true"],
            format!("which keeps {run}"),
        ),
        (
            &["inspect", run, "before", "db", "--", "true"],
            "on the before side".to_owned(),
        ),
        (
            &["inspect", run, "head", "api", "--", "true"],
            "`api`".to_owned(),
        ),
    ];
    for (args, said) in refusals {
        let ran = world.run(&fake, args);
        assert_eq!(ran.exit, Exit::Refused, "{args:?}");
        assert!(ran.err.contains(&said), "{said} in {}", ran.err);
        assert_eq!(ran.next(), ["stop-slop-stand guide inspect"], "{args:?}");
    }
    let created = |from: usize| {
        let journal = fake.journal();
        journal[from..]
            .iter()
            .any(|call| matches!(call, Call::Create { .. }))
    };
    assert!(!created(mark));
    // Ctrl+C before the copy starts: 130 and no VM.
    let cancel = Cancel::new();
    cancel.cancel();
    let args = ["inspect", run, "head", "db", "--", "true"];
    let ran = world.run_on(&fake, &args, &roomy, &cancel);
    assert_eq!(ran.exit, Exit::Cancelled, "{}", ran.err);
    assert!(ran.next().is_empty(), "{}", ran.err);
    assert!(!created(mark));
    // A `kept.json` that does not read breaks the stand.
    fs::write(world.run_dir(run).join("kept.json"), "[").unwrap();
    let ran = world.run(&fake, &args);
    assert_eq!(ran.exit, Exit::Broken, "{}", ran.err);
    assert_eq!(ran.next(), ["stop-slop-stand guide"]);
    // The session is the stand file's: without one there is no session to inspect.
    fs::remove_file(world.recipe().join("compose.yaml")).unwrap();
    let ran = world.run(&fake, &args);
    assert_eq!(ran.exit, Exit::Refused, "{}", ran.err);
    assert_eq!(ran.next(), ["stop-slop-stand init"]);
}

#[test]
fn inspect_of_a_service_that_does_not_start_alone_is_2() {
    // `db` never gets healthy: the head fails and keeps it, and its copy does not start either.
    let world = World::new();
    let unhealthy = Script {
        volume_bytes: 4096,
        health: vec![Some(1)],
        ..healthy()
    };
    let fake = fake(vec![rule("db", unhealthy)]);
    let failed = world.run(&fake, &["run", "--json"]);
    assert_eq!(failed.exit, Exit::AppFailed, "{}", failed.err);
    let run = failed.line()["run"].as_str().unwrap().to_owned();
    let ran = world.run(&fake, &["inspect", &run, "head", "db", "--", "true"]);
    assert_eq!(ran.exit, Exit::Refused, "{}", ran.err);
    assert!(
        ran.err
            .starts_with("db did not start in inspect: it stayed unhealthy"),
        "{}",
        ran.err
    );
    assert_eq!(ran.next(), ["stop-slop-stand guide inspect"]);
    assert!(fake.leftovers().unwrap().sandboxes.is_empty());
}

/// `accepted.json` of the one session of the world (`store`, a law of the blueprint).
fn acceptances(world: &World) -> Vec<Value> {
    let session = world.sessions().remove(0);
    let Ok(text) = fs::read_to_string(session.join("accepted.json")) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap()
}

#[test]
fn accept_appends_a_bound_with_its_reason_and_refuses_the_rest() {
    let world = World::new();
    let fake = fake(Vec::new());
    world.run(&fake, &["snapshot"]);
    let args = ["accept", "stand.wall", "--allow", "+15%", "--reason", "x"];
    let ran = world.run(&fake, &args);
    assert_eq!(ran.exit, Exit::Refused, "no run passed yet: {}", ran.err);
    assert_eq!(ran.next(), ["stop-slop-stand guide expect"]);
    world.repo.write("main.js", "console.log('v2')\n");
    let first = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    let tree = world.report(&first)["head"]["tree"]
        .as_str()
        .unwrap()
        .to_owned();
    // A bound is kept and printed in the grammar of `--allow`, as `next` writes it.
    let args = [
        "accept",
        "stand.wall",
        "--allow",
        "+015%",
        "--reason",
        "one more lookup",
    ];
    let ran = world.run(&fake, &args);
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    assert_eq!(
        ran.out,
        format!("accepted stand.wall +15% for head {tree}\n")
    );
    let args = [
        "accept",
        "stand.wall",
        "--allow",
        "-10%",
        "--reason",
        "a cache",
    ];
    assert_eq!(world.run(&fake, &args).exit, Exit::Passed);
    let accepted = acceptances(&world);
    let kept: Vec<(&str, &str, &str, &str)> = accepted
        .iter()
        .map(|entry| {
            let field = |key: &str| entry[key].as_str().unwrap();
            (
                field("measure"),
                field("allow"),
                field("reason"),
                field("headTree"),
            )
        })
        .collect();
    assert_eq!(
        kept,
        [
            ("stand.wall", "+15%", "one more lookup", tree.as_str()),
            ("stand.wall", "-10%", "a cache", tree.as_str()),
        ]
    );
    assert!(accepted.iter().all(|entry| entry["acceptedAt"].is_string()));
    // The next run of this head grades by the last acceptance, and counts both.
    let next = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    let report = world.report(&next);
    let wall = report["measures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|measure| measure["name"] == "stand.wall")
        .unwrap();
    let expect = &wall["expect"];
    assert_eq!(
        (
            &expect["change"],
            &expect["postHoc"],
            &expect["acceptances"]
        ),
        (&Value::from("-10%"), &Value::from(true), &Value::from(2)),
        "{expect}"
    );
    // One flag without the other, a bound wider than `max_tolerance`, `unknown`, a bound out of
    // the grammar and a measure the last report does not have: 2, and nothing appended.
    let refusals: [(&[&str], &[&str]); 6] = [
        (
            &["accept", "stand.wall", "--allow", "+15%"],
            &["stop-slop-stand guide"],
        ),
        (
            &["accept", "stand.wall", "--reason", "why"],
            &["stop-slop-stand guide"],
        ),
        (
            &[
                "accept",
                "stand.wall",
                "--allow",
                "+40%",
                "--reason",
                "wide",
            ],
            &[],
        ),
        (
            &[
                "accept",
                "stand.wall",
                "--allow",
                "unknown",
                "--reason",
                "x",
            ],
            &["stop-slop-stand guide expect"],
        ),
        (
            &["accept", "stand.wall", "--allow", "15%", "--reason", "x"],
            &["stop-slop-stand guide expect"],
        ),
        (
            &["accept", "no.such", "--allow", "+15%", "--reason", "x"],
            &["stop-slop-stand guide expect"],
        ),
    ];
    for (args, next) in refusals {
        let ran = world.run(&fake, args);
        assert_eq!(ran.exit, Exit::Refused, "{args:?}");
        assert_eq!(ran.next(), next, "{args:?}: {}", ran.err);
    }
    let wide = world.run(&fake, refusals[2].0);
    assert!(wide.err.contains("max_tolerance, 25%"), "{}", wide.err);
    assert_eq!(acceptances(&world).len(), 2);
}

#[test]
fn guide_prints_the_contents_and_each_topic_and_refuses_another() {
    let world = World::new();
    let fake = fake(Vec::new());
    // The guide needs no repository: an agent reads it before it has a stand file.
    let outside = tempfile::tempdir().unwrap();
    let outside = Utf8Path::from_path(outside.path()).unwrap();
    let contents = world.run_at(outside, &fake, &["guide"], &roomy, &Cancel::new());
    assert_eq!(contents.exit, Exit::Passed, "{}", contents.err);
    assert!(
        contents.out.contains("The shortest path"),
        "{}",
        contents.out
    );
    assert!(contents.out.contains("- `130`: "), "{}", contents.out);
    assert!(!contents.out.contains("## "), "{}", contents.out);
    let last = contents.out.lines().last();
    let topics = "Topics: recipe, probes, expect, line, inspect. `stop-slop-stand guide <topic>` prints one.";
    assert_eq!(last, Some(topics));
    for topic in ["recipe", "expect", "line", "inspect"] {
        let ran = world.run(&fake, &["guide", topic]);
        assert_eq!(ran.exit, Exit::Passed, "{topic}");
        let heading = format!("## {topic}");
        assert_eq!(ran.out.lines().next(), Some(heading.as_str()));
        let other = ran.out.lines().skip(1).find(|line| line.starts_with("## "));
        assert_eq!(other, None, "{topic} prints only its section");
    }
    let probes = world.run(&fake, &["guide", "probes"]);
    assert_eq!(probes.out.lines().next(), Some("# probes"));
    assert!(probes.out.contains("$$"));
    let unknown = world.run(&fake, &["guide", "nothing"]);
    assert_eq!(unknown.exit, Exit::Refused);
    assert!(
        unknown
            .err
            .contains("topics: recipe, probes, expect, line, inspect"),
        "{}",
        unknown.err
    );
    assert_eq!(unknown.next(), ["stop-slop-stand guide"]);
    assert!(fake.journal().is_empty());
}

#[test]
fn init_writes_a_template_that_loads_and_refuses_a_second_time() {
    let world = World::with("");
    let fake = fake(Vec::new());
    let ran = world.run(&fake, &["init"]);
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    let file = world.recipe().join("compose.yaml");
    let wrote = format!("wrote {}", file.as_str().replace('\\', "/"));
    assert_eq!(ran.out.trim_end(), wrote);
    assert!(spec::load(&file, &spec::Policy::new(None).unwrap()).is_ok());
    let dry = world.run(&fake, &["run", "--dry-run"]);
    assert_eq!(dry.exit, Exit::Passed, "{}", dry.err);
    let mine = format!("{}# mine\n", fs::read_to_string(&file).unwrap());
    fs::write(&file, &mine).unwrap();
    let again = world.run(&fake, &["init"]);
    assert_eq!(again.exit, Exit::Refused);
    assert!(again.err.contains("is there already"), "{}", again.err);
    assert_eq!(again.next(), ["stop-slop-stand guide recipe"]);
    assert_eq!(fs::read_to_string(&file).unwrap(), mine);
    assert!(!any_create(&fake));
    let outside = tempfile::tempdir().unwrap();
    let outside = Utf8Path::from_path(outside.path()).unwrap();
    let refused = world.run_at(outside, &fake, &["init"], &roomy, &Cancel::new());
    assert_eq!(refused.exit, Exit::Refused, "{}", refused.err);
    assert!(
        refused.err.contains("not inside a git work tree"),
        "{}",
        refused.err
    );
}

#[test]
fn help_holds_at_most_twenty_lines_and_ends_with_the_guide() {
    let world = World::new();
    let ran = world.run(&fake(Vec::new()), &["--help"]);
    assert_eq!(ran.exit, Exit::Passed);
    assert!(ran.out.lines().count() <= 20, "{}", ran.out);
    for command in [
        "run", "snapshot", "inspect", "accept", "guide", "init", "clean",
    ] {
        let listed = ran
            .out
            .lines()
            .any(|line| line.trim_start().starts_with(command));
        assert!(listed, "{command}: {}", ran.out);
    }
    assert_eq!(
        ran.out.trim_end().lines().last(),
        Some("agents: run `guide`")
    );
}

/// `main` runs `guide`, `init` and `snapshot` before it opens the stand folder or starts the
/// adapter (batch review 6): they work where the folder cannot be made, and `guide`, the `next`
/// of that failure, works after it.
#[test]
fn the_local_commands_open_no_stand_folder() {
    let world = World::with("");
    let base = Utf8Path::from_path(world.dir.path()).unwrap().to_owned();
    // A file where the stand folder's parent would be: no folder can be made under it.
    fs::write(base.join("file"), "").unwrap();
    let home = base.join("file").join("home");
    let stand = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_stop-slop-stand"))
            .args(args)
            .current_dir(&world.cwd)
            .env("STOP_SLOP_STAND_HOME", &home)
            .output()
            .unwrap()
    };
    let run = stand(&["run", "--json"]);
    assert_eq!(run.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.ends_with("next: stop-slop-stand guide\n"),
        "{stderr}"
    );
    for args in [
        &["--help"][..],
        &["guide"],
        &["guide", "recipe"],
        &["init"],
        &["snapshot"],
    ] {
        let local = stand(args);
        let stderr = String::from_utf8_lossy(&local.stderr);
        assert!(local.status.success(), "{args:?}: {stderr}");
    }
}
