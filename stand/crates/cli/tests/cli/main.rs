//! `execute` on a `Fake`, a temporary stand folder and a `testkit` repo whose stand file lies in
//! `<git common>/info/stop-slop-stand/`. The host is a value of the test: a CI runner has two cores
//! and less than the 20 GB the stand keeps free.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::time::{Duration, Instant};

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

mod accept;
mod cache;
mod dry_run;
mod exits;
mod inspect;
mod local;
mod pool;
mod seal;
mod sweep;

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

/// `<run>/inspect/<n>.out` as `inspect` prints it.
fn looked(world: &World, run: &str, n: u32) -> String {
    let out = world.run_dir(run).join("inspect").join(format!("{n}.out"));
    out.as_str().replace('\\', "/")
}
