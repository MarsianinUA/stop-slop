//! `run` and `inspect` on a `Fake` with its virtual clock. Each case loads a recipe with
//! `spec::load` (only `spec` builds its newtypes), runs it, and reads the journal of the `Fake`.
//! The cases go by phase, a file each; here are the recipes, `Case`, `Hooked` and the helpers
//! more than one file takes.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use report::{Extra, ProbeRan, Raw};
use run::{Cancel, RunError, SetupCache, Side, Version, Versions};
use sandbox::{
    Cached, Call, ExecId, ExecOutcome, Exited, Fake, Fault, ImageRef, Launch, Leftovers, Metrics,
    Mount, Pulled, Rule, Sandbox, SandboxError, SandboxId, Scenario, Script, Seed, SnapshotId,
    Stopped, VolumeName, Waitable,
};
use spec::{Policy, Stand};
use tempfile::TempDir;

mod cache;
mod keep;
mod outside;
mod probes;
mod profiles;
mod repeats;
mod setup;

/// The example of the plan, smaller: `db` with a volume and three probes, the prepare service,
/// `migrate`, two profiled replicas of `api` with a harness bind, the workload `load`.
const EXAMPLE: &str = r#"
services:
  db:
    image: postgres:17
    volumes: ["pgdata:/var/lib/postgresql/data"]
    expose: [5432]
    stop_signal: SIGINT
    stop_grace_period: 1m
    healthcheck: { test: ["CMD", "pg_isready"], interval: 1s, retries: 3 }
    deploy: { resources: { limits: { cpus: "1", memory: 1g } } }
  prepare:
    image: node:22
    working_dir: /w/app
    volumes: ["${STAND_REPO}:/w/app"]
    command: ["npm", "ci"]
    deploy: { resources: { limits: { cpus: "2", memory: 2g } } }
    x-stand: { network: public }
  migrate:
    image: node:22
    working_dir: /w/app
    volumes: ["${STAND_REPO}:/w/app"]
    command: ["node", "dist/migrate.js"]
    depends_on:
      prepare: { condition: service_completed_successfully }
      db: { condition: service_healthy }
    deploy: { resources: { limits: { cpus: "0.5", memory: 512m } } }
  api:
    image: node:22
    working_dir: /w/app
    volumes: ["${STAND_REPO}:/w/app", ".:/harness:ro"]
    command: ["node", "dist/main.js"]
    environment: { NODE_OPTIONS: "--max-old-space-size=512" }
    expose: [3000]
    stop_signal: SIGINT
    stop_grace_period: 5s
    depends_on:
      migrate: { condition: service_completed_successfully }
      db: { condition: service_healthy }
    healthcheck: { test: ["CMD", "node", "/harness/ping.js"], interval: 1s, retries: 3 }
    deploy: { resources: { limits: { cpus: "1.5", memory: 1g } }, replicas: 2 }
    x-stand: { profile: node }
  load:
    image: node:22
    working_dir: /harness
    volumes: [".:/harness:ro"]
    command: ["node", "load.js", "http://api:3000"]
    depends_on: { api: { condition: service_healthy } }
    deploy: { resources: { limits: { cpus: "1", memory: 512m } } }
volumes: { pgdata: {} }
x-stand:
  workload: load
  repeat: 3
  probes:
    schema: { in: db, at: after-setup, run: ["psql", "-At", "-c", "select 1"] }
    orders: { in: db, at: after-workload, run: ["psql", "-At", "-c", "select 2"], key: /id, ignore: [/at] }
    last: { in: db, at: on-failure, run: ["psql", "-At", "-c", "select 3"] }
"#;

const SECOND: Duration = Duration::from_secs(1);

/// A recipe folder with its harness, an empty run folder, the two tar files and two empty cache
/// entries, as `cli` and `store` would give them.
struct Case {
    _temp: TempDir,
    root: Utf8PathBuf,
    stand: Stand,
}

impl Case {
    fn new(recipe: &str) -> Case {
        let temp = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(temp.path()).unwrap().to_owned();
        let folder = root.join("recipe");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("compose.yaml"), recipe).unwrap();
        fs::write(folder.join("load.js"), "// load\n").unwrap();
        fs::write(folder.join("ping.js"), "// ping\n").unwrap();
        for dir in ["run", "entry-head", "entry-before"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        for tar in ["head.tar", "before.tar"] {
            fs::write(root.join(tar), b"").unwrap();
        }
        let stand = spec::load(&folder.join("compose.yaml"), &Policy::new(None).unwrap()).unwrap();
        Case {
            _temp: temp,
            root,
            stand,
        }
    }

    fn run_dir(&self) -> Utf8PathBuf {
        self.root.join("run")
    }

    fn tar(&self, side: Side) -> Utf8PathBuf {
        self.root.join(format!("{side}.tar"))
    }

    fn entry(&self, side: Side) -> Utf8PathBuf {
        self.root.join(format!("entry-{side}"))
    }

    /// The snapshots the entries list: a run leaves them to `cli`, which drops them with an entry.
    fn cached(&self) -> Vec<SnapshotId> {
        let sides = [Side::Head, Side::Before];
        let lists = sides.map(|side| run::cached_ids(&self.entry(side)).unwrap());
        lists.into_iter().flatten().flatten().collect()
    }

    /// The snapshots `kept.json` of the run folder lists.
    fn kept(&self) -> Vec<SnapshotId> {
        run::kept_ids(&self.run_dir()).unwrap()
    }

    /// What a run leaves: the snapshots of the entries and of `kept.json`, and nothing else.
    fn left(&self) -> Vec<SnapshotId> {
        let mut left = self.cached();
        left.extend(self.kept());
        left
    }

    fn version(&self, side: Side) -> Version {
        Version {
            tar: self.tar(side),
            setup: SetupCache::Build(self.entry(side)),
        }
    }

    fn versions(&self, before: bool) -> Versions {
        Versions {
            head: self.version(Side::Head),
            before: before.then(|| self.version(Side::Before)),
        }
    }

    /// With every extra: what the stand asks for runs.
    fn run(&self, sandbox: &impl Sandbox, versions: &Versions) -> Result<Raw, RunError> {
        self.run_with(sandbox, versions, &every_extra())
    }

    fn run_with(
        &self,
        sandbox: &impl Sandbox,
        versions: &Versions,
        extras: &BTreeSet<Extra>,
    ) -> Result<Raw, RunError> {
        let run_dir = self.run_dir();
        run::run(
            &self.stand,
            sandbox,
            versions,
            extras,
            &run_dir,
            &Cancel::new(),
        )
    }
}

fn every_extra() -> BTreeSet<Extra> {
    BTreeSet::from([Extra::Coverage, Extra::Fault, Extra::SecondPass])
}

fn rule(service: &str, script: Script) -> Rule {
    Rule {
        service: service.into(),
        tree: None,
        script,
    }
}

fn exits(code: i32, after: Duration) -> Script {
    Script {
        exit: Some((code, after)),
        ..Script::default()
    }
}

fn stops(after: Duration) -> Script {
    Script {
        health: vec![Some(0)],
        stop: Stopped {
            after,
            killed: false,
        },
        ..Script::default()
    }
}

fn probe(file: &str, bytes: &[u8]) -> (String, ExecOutcome, Vec<u8>) {
    (file.into(), ExecOutcome::Exited(0), bytes.to_vec())
}

/// The scripts of `EXAMPLE` where everything passes: `prepare` takes 20 s, `migrate` 2 s, `load` 30 s.
fn example_rules() -> Vec<Rule> {
    vec![
        rule(
            "db",
            Script {
                volume_bytes: 4096,
                probes: vec![
                    probe("probe.schema.jsonl", b"{\"table\":\"orders\"}\n"),
                    probe("probe.orders.jsonl", b"{\"id\":1}\n"),
                ],
                ..stops(300 * Duration::from_millis(1))
            },
        ),
        rule("prepare", exits(0, 20 * SECOND)),
        rule("migrate", exits(0, 2 * SECOND)),
        rule("api", stops(100 * Duration::from_millis(1))),
        rule(
            "load",
            Script {
                writes: vec![(
                    "/stand/out/orders.jsonl".into(),
                    b"{\"ok\":true}\n".to_vec(),
                )],
                ..exits(0, 30 * SECOND)
            },
        ),
    ]
}

/// `rules` with `script` for the VMs of `service` on the tree of "before" (the first rule
/// that matches wins).
fn on_before(case: &Case, mut rules: Vec<Rule>, service: &str, script: Script) -> Vec<Rule> {
    let before = Rule {
        service: service.into(),
        tree: Some(case.tar(Side::Before)),
        script,
    };
    rules.insert(0, before);
    rules
}

fn replaced(mut rules: Vec<Rule>, service: &str, script: Script) -> Vec<Rule> {
    rules.retain(|rule| rule.service != service);
    rules.push(rule(service, script));
    rules
}

/// `example_rules` with the script of `service` replaced.
fn with(service: &str, script: Script) -> Vec<Rule> {
    replaced(example_rules(), service, script)
}

fn fake(rules: Vec<Rule>) -> Fake {
    Fake::new(Scenario {
        rules,
        ..Scenario::default()
    })
}

/// Every `Create` of the journal, flattened: the launch and the id it made.
fn created(journal: &[Call]) -> Vec<(Launch, SandboxId)> {
    let creates = journal.iter().filter_map(|call| match call {
        Call::Create { batch, ids } => Some(batch.iter().cloned().zip(ids.iter().cloned())),
        _ => None,
    });
    creates.flatten().collect()
}

fn index_of(journal: &[Call], matches: impl Fn(&Call) -> bool) -> usize {
    let found = journal.iter().position(matches);
    found.unwrap_or_else(|| panic!("no such call in {journal:#?}"))
}

/// Every VM is destroyed, its logs and metrics read before its `destroy`; no VM is left, and the
/// snapshots left are exactly `kept`.
fn assert_clean(fake: &Fake, kept: &[SnapshotId]) {
    let journal = fake.journal();
    for (launch, id) in created(&journal) {
        let gone = index_of(
            &journal,
            |call| matches!(call, Call::Destroy(made) if *made == id),
        );
        let logs = index_of(
            &journal,
            |call| matches!(call, Call::Logs(made) if *made == id),
        );
        let metrics = index_of(
            &journal,
            |call| matches!(call, Call::Metrics(made) if *made == id),
        );
        assert!(
            logs < gone && metrics < gone,
            "{} read after destroy",
            launch.service
        );
    }
    assert_left(fake, kept);
}

/// No VM is left, and the snapshots left are exactly `kept`: also after `Cancel` or a stand
/// fault, which read no metrics.
fn assert_left(fake: &Fake, kept: &[SnapshotId]) {
    let Leftovers {
        sandboxes,
        snapshots,
    } = fake.leftovers().unwrap();
    assert!(sandboxes.is_empty(), "{sandboxes:?}");
    let left: BTreeSet<String> = snapshots.iter().map(|id| id.to_string()).collect();
    let kept: BTreeSet<String> = kept.iter().map(|id| id.to_string()).collect();
    assert_eq!(left, kept);
}

fn app_failure(result: Result<Raw, RunError>) -> Box<run::Failed> {
    match result {
        Err(RunError::App(failed)) => failed,
        other => panic!("not an app failure: {other:?}"),
    }
}

fn names_in(dir: &Utf8Path) -> Vec<String> {
    let names = fs::read_dir(dir).unwrap().map(|entry| {
        let name = entry.unwrap().file_name();
        name.into_string().unwrap()
    });
    let mut names: Vec<String> = names.collect();
    names.sort();
    names
}

/// The instances of `dir/metrics.json`.
fn metrics_in(dir: &Utf8Path) -> Vec<serde_json::Value> {
    let text = fs::read_to_string(dir.join("metrics.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// The service of each instance, and whether it has `stopped`.
fn stopped(metrics: &[serde_json::Value]) -> Vec<(&str, bool)> {
    let instances = metrics.iter().map(|instance| {
        let service = instance["service"].as_str().unwrap();
        (service, !instance["stopped"].is_null())
    });
    instances.collect()
}

/// The files under `dir` with their bytes, by their path from it.
fn files_in(dir: &Utf8Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    for name in names_in(dir) {
        let path = dir.join(&name);
        if path.is_dir() {
            let nested = files_in(&path).into_iter();
            files.extend(nested.map(|(inner, bytes)| (format!("{name}/{inner}"), bytes)));
        } else {
            files.insert(name, fs::read(&path).unwrap());
        }
    }
    files
}

/// No prepare service: the profiled `api` runs in setup, for `fill`, and in every repeat; the
/// setup service `fill` says `profile` too. No setup service mounts the tree.
const NO_PREPARE: &str = r#"
services:
  api:
    image: node:22
    volumes: ["${STAND_REPO}:/w/app"]
    expose: [3000]
    healthcheck: { test: ["CMD", "true"], interval: 1s }
    deploy: { resources: { limits: { cpus: "1", memory: 512m } } }
    x-stand: { profile: node }
  fill:
    image: node:22
    command: ["node", "fill.js", "http://api:3000"]
    depends_on: { api: { condition: service_healthy } }
    deploy: { resources: { limits: { cpus: "1", memory: 512m } } }
    x-stand: { profile: node }
  load:
    image: node:22
    depends_on:
      api: { condition: service_healthy }
      fill: { condition: service_completed_successfully }
    deploy: { resources: { limits: { cpus: "1", memory: 512m } } }
x-stand: { workload: load, repeat: 2 }
"#;

fn no_prepare_rules() -> Vec<Rule> {
    vec![
        rule("api", stops(SECOND)),
        rule("fill", exits(0, 2 * SECOND)),
        rule("load", exits(0, 5 * SECOND)),
    ]
}

/// The `Fake` with what a script cannot say: `cancel` set once a `create` starts `cancel_at`
/// (Ctrl+C in the middle of a phase); `hang`: the `n`-th VM of a service, from 1, never ends,
/// its exit kept out of every `wait`; `fail_stop`: every `stop` is a `Backend` error, and
/// `fail_fault`: the `n`-th `fault`, from 1, and every later one; `logs_grow`: each `logs` of a
/// VM is one line longer than the one before, as the output of a running service grows;
/// `latency`: a `wait` that saw an exit returns this much after it, as the adapter's poll does,
/// while `Exited.at` keeps when it ended; `entry`: a file whose presence each `create` and
/// `destroy` notes in `seen`, by VM; `ends`: VMs that end on their own.
struct Hooked<'f> {
    fake: &'f Fake,
    cancel_at: Option<&'static str>,
    cancel: Cancel,
    hang: Option<(&'static str, usize)>,
    fail_stop: bool,
    fail_fault: Option<usize>,
    /// `fault` calls so far.
    switched: Cell<usize>,
    logs_grow: bool,
    latency: Duration,
    ends: Vec<End>,
    /// VMs so far, by service.
    made: RefCell<BTreeMap<String, usize>>,
    hidden: RefCell<Vec<SandboxId>>,
    /// The VMs of `ends` once made and not yet ended: when each ends, and its code.
    ending: RefCell<Vec<(SandboxId, Instant, Option<i32>)>>,
    /// `logs` calls so far, by VM.
    reads: RefCell<Vec<(SandboxId, usize)>>,
    /// The time each `exec` had left at its call.
    deadlines: RefCell<Vec<Duration>>,
    entry: Option<Utf8PathBuf>,
    seen: RefCell<Vec<(SandboxId, bool)>>,
}

/// The `nth` VM of `service`, from 1, ends with `code` that long after its `create`, whatever its
/// script says; `None` is a kill without a code.
#[derive(Debug, Clone, Copy)]
struct End {
    service: &'static str,
    nth: usize,
    code: Option<i32>,
    after: Duration,
}

fn end(service: &'static str, nth: usize, code: i32, after: Duration) -> End {
    End {
        service,
        nth,
        code: Some(code),
        after,
    }
}

fn killed(service: &'static str, nth: usize, after: Duration) -> End {
    End {
        code: None,
        ..end(service, nth, 0, after)
    }
}

impl<'f> Hooked<'f> {
    fn new(fake: &'f Fake) -> Hooked<'f> {
        Hooked {
            fake,
            cancel_at: None,
            cancel: Cancel::new(),
            hang: None,
            fail_stop: false,
            fail_fault: None,
            switched: Cell::new(0),
            logs_grow: false,
            latency: Duration::ZERO,
            ends: Vec::new(),
            made: RefCell::new(BTreeMap::new()),
            hidden: RefCell::new(Vec::new()),
            ending: RefCell::new(Vec::new()),
            reads: RefCell::new(Vec::new()),
            deadlines: RefCell::new(Vec::new()),
            entry: None,
            seen: RefCell::new(Vec::new()),
        }
    }

    fn note(&self, id: &SandboxId) {
        if let Some(entry) = &self.entry {
            self.seen.borrow_mut().push((id.clone(), entry.exists()));
        }
    }

    fn run(&self, case: &Case, versions: &Versions) -> Result<Raw, RunError> {
        let (extras, run_dir) = (every_extra(), case.run_dir());
        run::run(&case.stand, self, versions, &extras, &run_dir, &self.cancel)
    }
}

impl Sandbox for Hooked<'_> {
    fn now(&self) -> Instant {
        self.fake.now()
    }
    fn pull(&self, images: &[ImageRef]) -> Result<Pulled, SandboxError> {
        self.fake.pull(images)
    }
    fn cached(&self, images: &[ImageRef]) -> Result<Cached, SandboxError> {
        self.fake.cached(images)
    }
    fn create(&self, batch: &[Launch]) -> Result<Vec<SandboxId>, SandboxError> {
        let ids = self.fake.create(batch)?;
        for (launch, id) in batch.iter().zip(&ids) {
            self.note(id);
            if self.cancel_at == Some(launch.service.as_str()) {
                self.cancel.cancel();
            }
            let n = {
                let mut made = self.made.borrow_mut();
                let count = made.entry(launch.service.clone()).or_default();
                *count += 1;
                *count
            };
            if self.hang == Some((launch.service.as_str(), n)) {
                self.hidden.borrow_mut().push(id.clone());
            }
            for end in &self.ends {
                if (end.service, end.nth) == (launch.service.as_str(), n) {
                    let at = self.fake.now() + end.after;
                    self.ending.borrow_mut().push((id.clone(), at, end.code));
                }
            }
        }
        Ok(ids)
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
        let hidden = self.hidden.borrow();
        let ending: Vec<(SandboxId, Instant, Option<i32>)> = self
            .ending
            .borrow()
            .iter()
            .filter(|(id, ..)| what.contains(&Waitable::Sandbox(id.clone())))
            .cloned()
            .collect();
        let own = |id: &SandboxId| ending.iter().any(|(ends, ..)| ends == id);
        let what: Vec<Waitable> = what
            .iter()
            .filter(
                |waited| !matches!(waited, Waitable::Sandbox(id) if hidden.contains(id) || own(id)),
            )
            .cloned()
            .collect();
        // Of VMs that end at one moment, the one made last comes first: a `wait` may give any.
        let first = ending.into_iter().rev().min_by_key(|(_, at, _)| *at);
        let until = first
            .as_ref()
            .map_or(deadline, |(_, at, _)| deadline.min(*at));
        let mut exited = self.fake.wait(&what, until)?;
        if exited.is_none()
            && let Some((id, at, code)) = first
            && at <= deadline
        {
            self.ending.borrow_mut().retain(|(ends, ..)| *ends != id);
            let what = Waitable::Sandbox(id);
            exited = Some(Exited { what, code, at });
        }
        if exited.is_some() && !self.latency.is_zero() {
            // Nothing to wait for: the `Fake` moves its clock to the deadline.
            self.fake.wait(&[], self.fake.now() + self.latency)?;
        }
        Ok(exited)
    }
    fn exec(
        &self,
        id: &SandboxId,
        argv: &[String],
        deadline: Instant,
        stdout: &Utf8Path,
    ) -> Result<ExecOutcome, SandboxError> {
        let left = deadline.saturating_duration_since(self.fake.now());
        self.deadlines.borrow_mut().push(left);
        self.fake.exec(id, argv, deadline, stdout)
    }
    fn stop(&self, id: &SandboxId) -> Result<Stopped, SandboxError> {
        if self.fail_stop {
            let message = format!("{id}: the guest agent is gone");
            return Err(SandboxError::Backend { message });
        }
        self.fake.stop(id)
    }
    fn fault(&self, ids: &[SandboxId], fault: &Fault) -> Result<(), SandboxError> {
        self.switched.set(self.switched.get() + 1);
        if self
            .fail_fault
            .is_some_and(|nth| self.switched.get() >= nth)
        {
            let message = "the relay is gone".to_owned();
            return Err(SandboxError::Backend { message });
        }
        self.fake.fault(ids, fault)
    }
    fn metrics(&self, id: &SandboxId) -> Result<Metrics, SandboxError> {
        self.fake.metrics(id)
    }
    fn logs(&self, id: &SandboxId) -> Result<Vec<u8>, SandboxError> {
        let bytes = self.fake.logs(id)?;
        if !self.logs_grow {
            return Ok(bytes);
        }
        let mut reads = self.reads.borrow_mut();
        let lines = match reads.iter_mut().find(|(read, _)| read == id) {
            Some((_, lines)) => {
                *lines += 1;
                *lines
            }
            None => {
                reads.push((id.clone(), 1));
                1
            }
        };
        let log: String = (1..=lines).map(|line| format!("{id} {line}\n")).collect();
        Ok(log.into_bytes())
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
        self.note(id);
        self.fake.destroy(id)
    }
    fn drop_snapshot(&self, snapshot: &SnapshotId) -> Result<(), SandboxError> {
        self.fake.drop_snapshot(snapshot)
    }
    fn leftovers(&self) -> Result<Leftovers, SandboxError> {
        self.fake.leftovers()
    }
    fn prune_images(&self, keep: &[ImageRef]) -> Result<(), SandboxError> {
        self.fake.prune_images(keep)
    }
}

/// The host folder of the `/stand/out` bind of `launch`.
fn out_of(launch: &Launch) -> Option<&Utf8Path> {
    launch.mounts.iter().find_map(|mount| match mount {
        Mount::Bind { host, guest, .. } if guest == "/stand/out" => Some(host.as_path()),
        _ => None,
    })
}

/// The folder of the phase whose `/stand/out` `launch` binds: `setup-head`, `repeat-2-before`,
/// `pass2-head` (for each `out-<n>/` of its passes).
fn phase_of(launch: &Launch) -> &str {
    let out = out_of(launch).expect("a bind of /stand/out");
    let mut folders = out.ancestors().filter_map(Utf8Path::file_name);
    folders.find(|name| !name.starts_with("out")).unwrap()
}

/// The folder of each phase that started the workload, by name, in order.
fn workload_folders(case: &Case, journal: &[Call]) -> Vec<String> {
    let made = created(journal);
    let loads = made
        .iter()
        .filter(|(launch, _)| launch.service == case.stand.workload.as_str());
    loads
        .map(|(launch, _)| phase_of(launch).to_owned())
        .collect()
}

/// `service-replica`.
fn vm(launch: &Launch) -> String {
    format!("{}-{}", launch.service, launch.replica)
}

/// `service-replica` of each VM, in the order of creation.
fn services(journal: &[Call]) -> Vec<String> {
    created(journal)
        .iter()
        .map(|(launch, _)| vm(launch))
        .collect()
}

/// From the first `wait` after the `create` of `id` to the deadline of the last one before its
/// `destroy`.
fn waited(journal: &[Call], id: &SandboxId) -> Duration {
    let start = index_of(
        journal,
        |call| matches!(call, Call::Create { ids, .. } if ids.contains(id)),
    );
    let end = index_of(
        journal,
        |call| matches!(call, Call::Destroy(gone) if gone == id),
    );
    let waits: Vec<Instant> = journal[start..end]
        .iter()
        .filter_map(|call| match call {
            Call::Wait { deadline, .. } => Some(*deadline),
            _ => None,
        })
        .collect();
    *waits.iter().max().unwrap() - waits[0]
}

fn logs_in(dir: &Utf8Path) -> BTreeSet<String> {
    let names = names_in(dir).into_iter();
    names.filter(|name| name.ends_with(".log")).collect()
}

fn names(logs: &[&str]) -> BTreeSet<String> {
    logs.iter().map(|log| format!("{log}.log")).collect()
}

/// `db`, then on one level the workload and `cache`, then `side`, which waits for `cache`: a
/// service whose `create` comes after the workload's.
const AFTER_THE_WORKLOAD: &str = r#"
services:
  db:
    image: postgres:17
    expose: [5432]
    healthcheck: { test: ["CMD", "pg_isready"], interval: 1s }
    deploy: { resources: { limits: { cpus: "1", memory: 1g } } }
  cache:
    image: redis:7
    depends_on: { db: { condition: service_healthy } }
    deploy: { resources: { limits: { cpus: "1", memory: 256m } } }
  side:
    image: busybox:1
    depends_on: { cache: { condition: service_started } }
    deploy: { resources: { limits: { cpus: "1", memory: 256m } } }
  load:
    image: node:22
    depends_on: { db: { condition: service_healthy } }
    deploy: { resources: { limits: { cpus: "1", memory: 512m } } }
x-stand: { workload: load }
"#;

/// Each `Exec` of `journal`: its index, its `stdout` and its VM as `service-replica`.
fn execs(journal: &[Call]) -> Vec<(usize, Utf8PathBuf, String)> {
    let made = created(journal);
    let mut execs = Vec::new();
    for (index, call) in journal.iter().enumerate() {
        if let Call::Exec { id, stdout, .. } = call {
            let (launch, _) = made.iter().find(|(_, made)| made == id).unwrap();
            execs.push((index, stdout.clone(), vm(launch)));
        }
    }
    execs
}

fn ran(probes: &[ProbeRan]) -> Vec<&str> {
    probes.iter().map(|probe| probe.name.as_str()).collect()
}

/// The host folder of the read-only `/stand/preload` bind of `launch`, and whether it has the
/// `Scratch` folder `/stand/profile`.
fn profile_mounts(launch: &Launch) -> (Option<&Utf8Path>, bool) {
    let preload = launch.mounts.iter().find_map(|mount| match mount {
        Mount::Bind {
            host,
            guest,
            read_only: true,
        } if guest == "/stand/preload" => Some(host.as_path()),
        _ => None,
    });
    let scratch = launch
        .mounts
        .iter()
        .any(|mount| matches!(mount, Mount::Scratch { guest } if guest == "/stand/profile"));
    (preload, scratch)
}

fn snapshot_of(journal: &[Call], id: &SandboxId) -> SnapshotId {
    let taken = journal.iter().find_map(|call| match call {
        Call::Snapshot { id: of, snapshot } if of == id => snapshot.clone(),
        _ => None,
    });
    taken.unwrap_or_else(|| panic!("no snapshot of {id}"))
}

/// The setup snapshot of `db` and the version snapshot that `case` took for `side`, by the
/// setup VMs of `journal`: `db` with its `/stand/out` in `setup-<side>`, `prepare` on its tree.
fn setup_snapshots(case: &Case, journal: &[Call], side: Side) -> (SnapshotId, SnapshotId) {
    let setup = case.run_dir().join(format!("setup-{side}"));
    let made = created(journal);
    let in_setup = |service: &str| {
        let found = made.iter().find(|(launch, _)| {
            launch.service == service
                && match &launch.seed {
                    Some(Seed::Tree { tar, .. }) => *tar == case.tar(side),
                    None => out_of(launch).is_some_and(|out| out.starts_with(&setup)),
                    Some(Seed::Snapshot(_)) => false,
                }
        });
        found
            .unwrap_or_else(|| panic!("{service} in the setup of {side}"))
            .1
            .clone()
    };
    let db = snapshot_of(journal, &in_setup("db"));
    (db, snapshot_of(journal, &in_setup("prepare")))
}

/// Each entry of `kept.json` in order: its side, its service and the folder of the repeat whose VM
/// gave the snapshot, by its `/stand/out`. No file is no entry; a file is never an empty list.
fn kept_from(case: &Case, journal: &[Call]) -> Vec<[String; 3]> {
    let path = case.run_dir().join("kept.json");
    if !path.exists() {
        return Vec::new();
    }
    let kept: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let kept = kept.as_array().unwrap();
    assert!(!kept.is_empty(), "an empty kept.json");
    let made = created(journal);
    let entries = kept.iter().map(|kept| {
        let snapshot = kept["snapshot"].as_str().unwrap();
        let vm = journal.iter().find_map(|call| match call {
            Call::Snapshot {
                id,
                snapshot: Some(own),
            } if own.as_str() == snapshot => Some(id),
            _ => None,
        });
        let (launch, _) = made.iter().find(|(_, made)| Some(made) == vm).unwrap();
        let text = |key: &str| kept[key].as_str().unwrap().to_owned();
        [text("side"), text("service"), phase_of(launch).to_owned()]
    });
    entries.collect()
}

/// `EXAMPLE` with more keys of the top-level `x-stand`, each line indented as they are.
fn with_x_stand(lines: &str) -> String {
    EXAMPLE.replace("  workload: load\n", &format!("  workload: load\n{lines}"))
}

/// `EXAMPLE` with the fault on `db:5432` and the second pass: the stand asks for every repeat
/// outside the stats.
fn every_repeat() -> String {
    with_x_stand("  fault: { target: \"db:5432\", mode: down }\n  second_pass: true\n")
}
