//! `run` on a `Fake` with its virtual clock. Each case loads a recipe with `spec::load` (only
//! `spec` builds its newtypes), runs it, and reads the journal of the `Fake`.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::fs;
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use report::{BeforeOutcome, Cause, Phase, ProbeRan, Raw, Role};
use run::{Cancel, RunError, SetupCache, Side, StandFault, Version, Versions};
use sandbox::{
    Call, ExecId, ExecOutcome, Exited, Fake, Fault, ImageRef, Launch, Leftovers, Metrics, Mount,
    Pulled, Rule, Sandbox, SandboxError, SandboxId, Scenario, Script, Seed, SnapshotId, Stopped,
    VolumeName, Waitable,
};
use spec::{Policy, Stand};
use tempfile::TempDir;

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

    fn run(&self, sandbox: &impl Sandbox, versions: &Versions) -> Result<Raw, RunError> {
        run::run(
            &self.stand,
            sandbox,
            versions,
            &self.run_dir(),
            &Cancel::new(),
        )
    }
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

/// `example_rules` with the script of `service` replaced.
fn with(service: &str, script: Script) -> Vec<Rule> {
    let mut rules = example_rules();
    rules.retain(|rule| rule.service != service);
    rules.push(rule(service, script));
    rules
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

// --- setup -------------------------------------------------------------------------------------

#[test]
fn every_stop_signal_spec_allows_has_a_number_in_stand_guest() {
    for name in spec::SIGNALS {
        assert!(guest::signal_number(name).is_some(), "{name}");
    }
}

#[test]
fn span_follows_the_formula_and_its_defaults_are_default_span() {
    let defaults = Case::new(
        r#"
services:
  load:
    image: node:22
    deploy: { resources: { limits: { cpus: "1", memory: 512m } } }
x-stand: { workload: load }
"#,
    );
    assert_eq!(run::span(&defaults.stand), run::DEFAULT_SPAN);
    // 10m x 2 + 5m x (2 x 3 + 1.5 for the profiled `api`) + 10m: fifteen halves of 150 s.
    let example = Case::new(EXAMPLE);
    assert_eq!(
        run::span(&example.stand),
        Duration::from_secs(20 * 60 + 15 * 150 + 10 * 60)
    );
    let extras = EXAMPLE.replace(
        "  workload: load\n",
        "  workload: load\n  fault: { target: \"db:5432\", mode: down }\n  second_pass: true\n",
    );
    // + 4 x 5m for the fault repeats, + 4 x 5m for the second pass.
    let extras = Case::new(&extras);
    assert_eq!(
        run::span(&extras.stand),
        run::span(&example.stand) + Duration::from_secs(40 * 60)
    );
}

#[test]
fn the_prepare_service_unpacks_the_tree_and_its_snapshot_seeds_the_other_tree_services() {
    let case = Case::new(EXAMPLE);
    let fake = fake(example_rules());
    case.run(&fake, &case.versions(true)).unwrap();
    let journal = fake.journal();
    let made = created(&journal);
    for side in [Side::Before, Side::Head] {
        let unpacks = |launch: &Launch| match &launch.seed {
            Some(Seed::Tree { tar, at }) => *tar == case.tar(side) && at == spec::TREE,
            _ => false,
        };
        let (_, prepare) = made
            .iter()
            .find(|(launch, _)| launch.service == "prepare" && unpacks(launch))
            .unwrap_or_else(|| panic!("prepare of the {side} side"));
        let snapshot = journal.iter().find_map(|call| match call {
            Call::Snapshot { id, snapshot } if id == prepare => snapshot.clone(),
            _ => None,
        });
        let version = snapshot.expect("a snapshot of the prepare VM");
        let restores =
            |launch: &Launch| matches!(&launch.seed, Some(Seed::Snapshot(id)) if *id == version);
        let seeded: BTreeSet<&str> = made
            .iter()
            .filter(|(launch, _)| restores(launch))
            .map(|(launch, _)| launch.service.as_str())
            .collect();
        assert_eq!(seeded, BTreeSet::from(["api", "migrate"]), "{side}");
        let api = made
            .iter()
            .filter(|(launch, _)| launch.service == "api" && restores(launch));
        assert_eq!(api.count(), 2 * 3, "replicas x repeats");
        // The version snapshot comes after the prepare service exited, before `migrate` starts.
        let taken = index_of(
            &journal,
            |call| matches!(call, Call::Snapshot { id, .. } if id == prepare),
        );
        let migrate = index_of(
            &journal,
            |call| matches!(call, Call::Create { batch, .. } if batch.iter().any(restores)),
        );
        assert!(taken < migrate);
    }
    assert_clean(&fake, &[]);
}

#[test]
fn without_a_prepare_service_every_tree_service_unpacks_the_tree() {
    // `api` runs in setup, for `fill`, and in both repeats; no setup service mounts the tree.
    let recipe = r#"
services:
  api:
    image: node:22
    volumes: ["${STAND_REPO}:/w/app"]
    expose: [3000]
    healthcheck: { test: ["CMD", "true"], interval: 1s }
    deploy: { resources: { limits: { cpus: "1", memory: 512m } } }
  fill:
    image: node:22
    command: ["node", "fill.js", "http://api:3000"]
    depends_on: { api: { condition: service_healthy } }
    deploy: { resources: { limits: { cpus: "1", memory: 512m } } }
  load:
    image: node:22
    depends_on:
      api: { condition: service_healthy }
      fill: { condition: service_completed_successfully }
    deploy: { resources: { limits: { cpus: "1", memory: 512m } } }
x-stand: { workload: load, repeat: 2 }
"#;
    let case = Case::new(recipe);
    assert!(case.stand.prepare.is_none());
    let fake = fake(vec![
        rule("api", stops(SECOND)),
        rule("fill", exits(0, 2 * SECOND)),
        rule("load", exits(0, 5 * SECOND)),
    ]);
    case.run(&fake, &case.versions(false)).unwrap();
    let journal = fake.journal();
    let api: Vec<_> = created(&journal)
        .into_iter()
        .filter(|(launch, _)| launch.service == "api")
        .collect();
    assert_eq!(api.len(), 1 + 2);
    for (launch, _) in api {
        assert!(matches!(launch.seed, Some(Seed::Tree { tar, .. }) if tar == case.tar(Side::Head)));
    }
    assert!(
        !journal
            .iter()
            .any(|call| matches!(call, Call::Snapshot { .. }))
    );
    assert_clean(&fake, &[]);
}

#[test]
fn setup_stops_each_volume_vm_then_takes_its_size_and_snapshot() {
    let case = Case::new(EXAMPLE);
    let fake = fake(example_rules());
    let raw = case.run(&fake, &case.versions(false)).unwrap();
    assert_eq!(raw.setup.head.sizes, [("pgdata".to_owned(), 4096)].into());
    let journal = fake.journal();
    let made = created(&journal);
    let mut snapshots = 0;
    for (index, call) in journal.iter().enumerate() {
        let Call::Snapshot { id, .. } = call else {
            continue;
        };
        let (launch, _) = made.iter().find(|(_, made)| made == id).unwrap();
        if launch.service == "prepare" {
            continue;
        }
        snapshots += 1;
        let before = &journal[..index];
        assert!(
            before
                .iter()
                .any(|call| matches!(call, Call::Stop(made) if made == id)),
            "stop before the snapshot of {}",
            launch.service
        );
        assert!(
            before
                .iter()
                .any(|call| matches!(call, Call::VolumeSizes(made) if made == id))
        );
    }
    assert_eq!(snapshots, 1, "the snapshot of db");
    // Only the long-running `db` is stopped; the setup services have exited.
    let text = fs::read_to_string(case.run_dir().join("setup-head/metrics.json")).unwrap();
    let metrics: serde_json::Value = serde_json::from_str(&text).unwrap();
    let stopped: Vec<(&str, bool)> = metrics
        .as_array()
        .unwrap()
        .iter()
        .map(|instance| {
            let service = instance["service"].as_str().unwrap();
            (service, !instance["stopped"].is_null())
        })
        .collect();
    assert_eq!(
        stopped,
        [("db", true), ("prepare", false), ("migrate", false)]
    );
    assert!(case.run_dir().join("setup-head/db-1.log").is_file());
    assert_clean(&fake, &[]);
}

#[test]
fn a_failed_setup_is_an_app_failure_that_drops_its_snapshots_and_writes_no_entry() {
    let case = Case::new(EXAMPLE);
    let fake = fake(with("migrate", exits(1, 2 * SECOND)));
    let failed = app_failure(case.run(&fake, &case.versions(false)));
    let failure = &failed.failure;
    assert_eq!(
        (failure.phase, failure.service.as_deref(), &failure.cause),
        (Phase::Setup, Some("migrate"), &Cause::Exit(1))
    );
    assert_eq!(
        failure.logs.as_deref(),
        Some(case.run_dir().join("setup-head").as_path())
    );
    assert!(
        case.run_dir()
            .join("setup-head")
            .join("migrate-1.log")
            .is_file()
    );
    assert!(failed.kept.is_empty());
    assert!(failed.raw.head.is_empty());
    let journal = fake.journal();
    let version = journal.iter().find_map(|call| match call {
        Call::Snapshot { snapshot, .. } => snapshot.clone(),
        _ => None,
    });
    let version = version.expect("the version snapshot");
    assert!(
        journal
            .iter()
            .any(|call| matches!(call, Call::DropSnapshot(id) if *id == version))
    );
    assert_clean(&fake, &[]);
}

#[test]
fn setup_fails_at_its_timeout_and_on_an_unhealthy_database() {
    let case = Case::new(EXAMPLE);
    let hung = fake(with("prepare", Script::default()));
    let start = hung.now();
    let failure = app_failure(case.run(&hung, &case.versions(false))).failure;
    assert_eq!(
        (failure.phase, failure.service.as_deref(), failure.cause),
        (Phase::Setup, Some("prepare"), Cause::SetupTimeout)
    );
    assert_eq!(hung.now() - start, 10 * 60 * SECOND);
    assert_clean(&hung, &[]);

    let case = Case::new(EXAMPLE);
    let sick = fake(with(
        "db",
        Script {
            health: vec![Some(1)],
            ..Script::default()
        },
    ));
    let failure = app_failure(case.run(&sick, &case.versions(false))).failure;
    assert_eq!(
        (failure.phase, failure.service.as_deref(), failure.cause),
        (Phase::Setup, Some("db"), Cause::Unhealthy)
    );
    assert_clean(&sick, &[]);

    // Each attempt hangs until the guest kills it at the default `timeout` of 30 s: three failed
    // attempts with 1 s between them.
    let case = Case::new(EXAMPLE);
    let hanging = fake(with(
        "db",
        Script {
            health: vec![None],
            ..Script::default()
        },
    ));
    let start = hanging.now();
    let failure = app_failure(case.run(&hanging, &case.versions(false))).failure;
    assert_eq!(
        (failure.service.as_deref(), failure.cause),
        (Some("db"), Cause::Unhealthy)
    );
    assert_eq!(hanging.now() - start, (3 * 30 + 2) * SECOND);
    let attempts: Vec<Duration> = hanging
        .journal()
        .iter()
        .filter_map(|call| match call {
            Call::ExecDetached { timeout, .. } => Some(*timeout),
            _ => None,
        })
        .collect();
    assert_eq!(attempts, [30 * SECOND; 3]);
    assert_clean(&hanging, &[]);

    // The deadline cuts the second attempt short: that is the setup timeout, not its last retry.
    let recipe = EXAMPLE
        .replace(
            r#"["CMD", "pg_isready"], interval: 1s, retries: 3 }"#,
            r#"["CMD", "pg_isready"], interval: 1s, retries: 2 }"#,
        )
        .replace("  repeat: 3\n", "  repeat: 3\n  setup_timeout: 50s\n");
    let case = Case::new(&recipe);
    let cut = fake(with(
        "db",
        Script {
            health: vec![None],
            ..Script::default()
        },
    ));
    let start = cut.now();
    let failure = app_failure(case.run(&cut, &case.versions(false))).failure;
    assert_eq!(
        (failure.service.as_deref(), failure.cause),
        (Some("db"), Cause::SetupTimeout)
    );
    assert_eq!(cut.now() - start, 50 * SECOND);
    assert_clean(&cut, &[]);

    // Failures before `start_period` do not count: seven fail, the five in its first 5 s are free.
    let case = Case::new(&EXAMPLE.replace(
        r#"["CMD", "pg_isready"], interval: 1s, retries: 3 }"#,
        r#"["CMD", "pg_isready"], interval: 1s, retries: 3, start_period: 5s }"#,
    ));
    let mut health = vec![Some(1); 7];
    health.push(Some(0));
    let starting = fake(with(
        "db",
        Script {
            health,
            ..stops(SECOND)
        },
    ));
    case.run(&starting, &case.versions(false)).unwrap();
    assert_clean(&starting, &[]);
}

#[test]
fn a_healthcheck_attempt_waits_with_every_running_vm_and_a_neighbour_dies_in_it() {
    let case = Case::new(EXAMPLE);
    let fake = fake(example_rules());
    case.run(&fake, &case.versions(false)).unwrap();
    let journal = fake.journal();
    let first = index_of(&journal, |call| matches!(call, Call::ExecDetached { .. }));
    let Call::ExecDetached { id: db, argv, .. } = &journal[first] else {
        unreachable!()
    };
    assert_eq!(argv, &["pg_isready"]);
    let Call::Wait { what, .. } = &journal[first + 1] else {
        panic!("a wait follows the attempt")
    };
    assert!(
        what.iter()
            .any(|waited| matches!(waited, Waitable::Exec(_)))
    );
    assert!(what.contains(&Waitable::Sandbox(db.clone())));
    let prepare = created(&journal)
        .into_iter()
        .find(|(launch, _)| launch.service == "prepare")
        .unwrap()
        .1;
    assert!(
        what.contains(&Waitable::Sandbox(prepare)),
        "the prepare VM runs beside db"
    );
    assert_clean(&fake, &[]);

    // `db` never gets healthy and `prepare` crashes meanwhile: the crash ends setup, not the deadline.
    let case = Case::new(EXAMPLE);
    let mut rules = with(
        "db",
        Script {
            health: vec![Some(1)],
            ..Script::default()
        },
    );
    rules.retain(|rule| rule.service != "prepare");
    rules.push(rule("prepare", exits(2, SECOND)));
    let crashing = crate::fake(rules);
    let failure = app_failure(case.run(&crashing, &case.versions(false))).failure;
    assert_eq!(
        (failure.service.as_deref(), failure.cause),
        (Some("prepare"), Cause::Exit(2))
    );
    assert_clean(&crashing, &[]);
}

#[test]
fn each_launch_has_span_as_its_max_duration_and_the_stop_of_its_service() {
    let case = Case::new(EXAMPLE);
    let fake = fake(example_rules());
    case.run(&fake, &case.versions(false)).unwrap();
    for (launch, _) in created(&fake.journal()) {
        assert_eq!(launch.max_duration, run::span(&case.stand));
        let (signal, grace, cpus) = match launch.service.as_str() {
            "db" => (Some("SIGINT"), 60 * SECOND, 1),
            "api" => (Some("SIGINT"), 5 * SECOND, 2),
            "prepare" => (None, 10 * SECOND, 2),
            _ => (None, 10 * SECOND, 1),
        };
        let stop = (launch.stop_signal.as_deref(), launch.stop_grace);
        assert_eq!(stop, (signal, grace), "{}", launch.service);
        assert_eq!(
            launch.cpus, cpus,
            "{}: whole vCPUs, rounded up",
            launch.service
        );
        assert_eq!(launch.peers.len(), 5, "every service resolves");
    }
    assert_clean(&fake, &[]);
}

/// The `Fake` with what a script cannot say: `cancel` set once a `create` starts `cancel_at`
/// (Ctrl+C in the middle of a phase); `hang`: the `n`-th VM of a service, from 1, never ends,
/// its exit kept out of every `wait`; `fail_stop`: every `stop` is a `Backend` error;
/// `logs_grow`: each `logs` of a VM is one line longer than the one before, as the output of a
/// running service grows; `latency`: a `wait` that saw an exit returns this much after it, as
/// the adapter's poll does, while `Exited.at` keeps when it ended.
struct Hooked<'f> {
    fake: &'f Fake,
    cancel_at: Option<&'static str>,
    cancel: Cancel,
    hang: Option<(&'static str, usize)>,
    fail_stop: bool,
    logs_grow: bool,
    latency: Duration,
    /// VMs of the service of `hang` so far.
    made: Cell<usize>,
    hidden: RefCell<Vec<SandboxId>>,
    /// `logs` calls so far, by VM.
    reads: RefCell<Vec<(SandboxId, usize)>>,
    /// The time each `exec` had left at its call.
    deadlines: RefCell<Vec<Duration>>,
}

impl<'f> Hooked<'f> {
    fn new(fake: &'f Fake) -> Hooked<'f> {
        Hooked {
            fake,
            cancel_at: None,
            cancel: Cancel::new(),
            hang: None,
            fail_stop: false,
            logs_grow: false,
            latency: Duration::ZERO,
            made: Cell::new(0),
            hidden: RefCell::new(Vec::new()),
            reads: RefCell::new(Vec::new()),
            deadlines: RefCell::new(Vec::new()),
        }
    }

    fn run(&self, case: &Case, versions: &Versions) -> Result<Raw, RunError> {
        run::run(&case.stand, self, versions, &case.run_dir(), &self.cancel)
    }
}

impl Sandbox for Hooked<'_> {
    fn now(&self) -> Instant {
        self.fake.now()
    }
    fn pull(&self, images: &[ImageRef]) -> Result<Pulled, SandboxError> {
        self.fake.pull(images)
    }
    fn create(&self, batch: &[Launch]) -> Result<Vec<SandboxId>, SandboxError> {
        let ids = self.fake.create(batch)?;
        for (launch, id) in batch.iter().zip(&ids) {
            if self.cancel_at == Some(launch.service.as_str()) {
                self.cancel.cancel();
            }
            if let Some((service, n)) = self.hang
                && launch.service == service
            {
                self.made.set(self.made.get() + 1);
                if self.made.get() == n {
                    self.hidden.borrow_mut().push(id.clone());
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
        let what: Vec<Waitable> = what
            .iter()
            .filter(|waited| !matches!(waited, Waitable::Sandbox(id) if hidden.contains(id)))
            .cloned()
            .collect();
        let exited = self.fake.wait(&what, deadline)?;
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

#[test]
fn a_stand_fault_or_cancel_in_setup_tears_it_down_in_the_setup_phase() {
    // `migrate` has an image the registry lacks: its `create` fails once `db` runs.
    let case = Case::new(
        r#"
services:
  db:
    image: postgres:17
    expose: [5432]
    healthcheck: { test: ["CMD", "pg_isready"], interval: 1s }
    deploy: { resources: { limits: { cpus: "1", memory: 1g } } }
  migrate:
    image: flyway:10
    depends_on: { db: { condition: service_healthy } }
    deploy: { resources: { limits: { cpus: "1", memory: 512m } } }
  load:
    image: node:22
    depends_on: { migrate: { condition: service_completed_successfully } }
    deploy: { resources: { limits: { cpus: "1", memory: 512m } } }
x-stand: { workload: load }
"#,
    );
    let fault = Fake::new(Scenario {
        rules: vec![rule("db", stops(SECOND))],
        missing_images: vec!["flyway:10".into()],
        ..Scenario::default()
    });
    let result = case.run(&fault, &case.versions(false));
    assert!(
        matches!(
            &result,
            Err(RunError::Stand {
                phase: Phase::Setup,
                fault: StandFault::Sandbox(SandboxError::ImageNotFound(_)),
            })
        ),
        "{result:?}"
    );
    assert!(case.run_dir().join("setup-head/db-1.log").is_file());
    assert_left(&fault, &[]);

    // Ctrl+C once `migrate` is created: the version snapshot is taken, and it goes too.
    let case = Case::new(EXAMPLE);
    let fake = fake(example_rules());
    let sandbox = Hooked {
        cancel_at: Some("migrate"),
        ..Hooked::new(&fake)
    };
    let result = sandbox.run(&case, &case.versions(true));
    assert!(
        matches!(
            result,
            Err(RunError::Cancelled {
                phase: Phase::Setup
            })
        ),
        "{result:?}"
    );
    let setup = case.run_dir().join("setup-before");
    for log in ["db-1.log", "prepare-1.log", "migrate-1.log"] {
        assert!(setup.join(log).is_file(), "{log}");
    }
    assert!(!case.run_dir().join("setup-head").exists());
    let journal = fake.journal();
    assert!(
        journal
            .iter()
            .any(|call| matches!(call, Call::Snapshot { .. }))
    );
    assert_left(&fake, &[]);
}

#[test]
fn a_failure_of_before_stops_only_its_lane() {
    let case = Case::new(EXAMPLE);
    let mut rules = example_rules();
    // `migrate` of the before tree fails; the head's passes.
    rules.insert(
        0,
        Rule {
            service: "migrate".into(),
            tree: Some(case.tar(Side::Before)),
            script: exits(1, SECOND),
        },
    );
    let fake = fake(rules);
    let raw = case.run(&fake, &case.versions(true)).unwrap();
    let Some(BeforeOutcome::Failed(failure)) = &raw.before else {
        panic!("{:?}", raw.before)
    };
    assert_eq!(
        (failure.phase, failure.service.as_deref()),
        (Phase::Setup, Some("migrate"))
    );
    assert_eq!(raw.setup.head.sizes, [("pgdata".to_owned(), 4096)].into());
    // The version snapshot of the failed setup goes at once, before the head's setup starts.
    let journal = fake.journal();
    let made = created(&journal);
    let prepare = |side: Side| {
        let found = made.iter().find(|(launch, _)| {
            launch.service == "prepare"
                && matches!(&launch.seed, Some(Seed::Tree { tar, .. }) if *tar == case.tar(side))
        });
        found.unwrap().1.clone()
    };
    let before = prepare(Side::Before);
    let version = journal.iter().find_map(|call| match call {
        Call::Snapshot { id, snapshot } if *id == before => snapshot.clone(),
        _ => None,
    });
    let version = version.expect("the version snapshot of before");
    let dropped = index_of(
        &journal,
        |call| matches!(call, Call::DropSnapshot(id) if *id == version),
    );
    let head = prepare(Side::Head);
    let head_starts = index_of(
        &journal,
        |call| matches!(call, Call::Create { ids, .. } if ids.contains(&head)),
    );
    assert!(dropped < head_starts);
    assert_eq!(raw.head.len(), 3);
    assert!(!case.run_dir().join("repeat-1-before").exists());
    assert_clean(&fake, &[]);

    // `api` of the before tree dies in its first repeat: "before" runs no other repeat.
    let case = Case::new(EXAMPLE);
    let mut rules = example_rules();
    rules.insert(
        0,
        Rule {
            service: "api".into(),
            tree: Some(case.tar(Side::Before)),
            script: Script {
                exit: Some((1, 5 * SECOND)),
                ..stops(SECOND)
            },
        },
    );
    let dying = crate::fake(rules);
    let raw = case.run(&dying, &case.versions(true)).unwrap();
    let Some(BeforeOutcome::Failed(failure)) = &raw.before else {
        panic!("{:?}", raw.before)
    };
    assert_eq!(
        (
            failure.phase,
            failure.service.as_deref(),
            &failure.cause,
            failure.repeat
        ),
        (Phase::Workload, Some("api"), &Cause::Died, Some(1))
    );
    assert_eq!(raw.head.len(), 3);
    let repeats = [
        "repeat-1-before",
        "repeat-1-head",
        "repeat-2-head",
        "repeat-3-head",
    ];
    assert_eq!(workload_folders(&case, &dying.journal()), repeats);
    assert!(case.run_dir().join("repeat-1-before/api-1.log").is_file());
    assert_clean(&dying, &[]);
}

// --- the measured repeats ----------------------------------------------------------------------

/// The host folder of the `/stand/out` bind of `launch`.
fn out_of(launch: &Launch) -> Option<&Utf8Path> {
    launch.mounts.iter().find_map(|mount| match mount {
        Mount::Bind { host, guest, .. } if guest == "/stand/out" => Some(host.as_path()),
        _ => None,
    })
}

/// The folder of each phase that started the workload, by name, in order.
fn workload_folders(case: &Case, journal: &[Call]) -> Vec<String> {
    let made = created(journal);
    let loads = made
        .iter()
        .filter(|(launch, _)| launch.service == case.stand.workload.as_str());
    let folders = loads.map(|(launch, _)| {
        let out = out_of(launch).expect("the workload binds /stand/out");
        out.parent().unwrap().file_name().unwrap().to_owned()
    });
    folders.collect()
}

/// `service-replica` of each VM, in the order of creation.
fn services(journal: &[Call]) -> Vec<String> {
    let made = created(journal);
    let names = made
        .iter()
        .map(|(launch, _)| format!("{}-{}", launch.service, launch.replica));
    names.collect()
}

#[test]
fn setup_runs_once_and_the_repeats_start_from_its_snapshots_by_levels() {
    let case = Case::new(EXAMPLE);
    let fake = fake(example_rules());
    let raw = case.run(&fake, &case.versions(false)).unwrap();
    assert!(raw.before.is_none() && raw.setup.before.is_none());
    let numbers: Vec<u32> = raw.head.iter().map(|output| output.repeat).collect();
    assert_eq!(numbers, [1, 2, 3]);
    let journal = fake.journal();
    // Setup, then each repeat by levels: `db`, the two `api`, `load`; setup services once.
    let repeat = ["db-1", "api-1", "api-2", "load-1"];
    let mut expected = vec!["db-1", "prepare-1", "migrate-1"];
    for _ in 0..3 {
        expected.extend(repeat);
    }
    assert_eq!(services(&journal), expected);
    let made = created(&journal);
    let db_setup = &made[0].1;
    let db_snapshot = journal.iter().find_map(|call| match call {
        Call::Snapshot { id, snapshot } if id == db_setup => snapshot.clone(),
        _ => None,
    });
    let db_snapshot = db_snapshot.expect("the setup snapshot of db");
    for (index, output) in raw.head.iter().enumerate() {
        let vms = &made[3 + 4 * index..][..4];
        let (db, _) = &vms[0];
        assert!(matches!(&db.seed, Some(Seed::Snapshot(id)) if *id == db_snapshot));
        // Each level after what it waits for: `api` after the healthcheck of this repeat's `db`,
        // `load` after those of both `api`.
        let created_at = |id: &SandboxId| {
            index_of(
                &journal,
                |call| matches!(call, Call::Create { ids, .. } if ids.contains(id)),
            )
        };
        let checked = |id: &SandboxId| {
            index_of(
                &journal,
                |call| matches!(call, Call::ExecDetached { id: on, .. } if on == id),
            )
        };
        assert!(checked(&vms[0].1) < created_at(&vms[1].1));
        assert!(checked(&vms[1].1) < created_at(&vms[3].1));
        assert!(checked(&vms[2].1) < created_at(&vms[3].1));
        let roles: Vec<(&str, u32, Role)> = output
            .instances
            .iter()
            .map(|instance| (instance.service.as_str(), instance.replica, instance.role))
            .collect();
        let long = Role::LongRunning;
        assert_eq!(
            roles,
            [
                ("db", 1, long),
                ("api", 1, long),
                ("api", 2, long),
                ("load", 1, Role::Workload)
            ]
        );
        let dir = case
            .run_dir()
            .join(format!("repeat-{}-head", output.repeat));
        assert_eq!(output.out_dir, dir.join("out"));
        assert_eq!(output.instances[1].log, dir.join("api-1.log"));
        assert!(output.instances[1].log.is_file());
        assert!(dir.join("metrics.json").is_file());
    }
    assert_clean(&fake, &[]);
}

#[test]
fn with_before_the_repeats_take_turns_and_the_harness_is_the_same() {
    let case = Case::new(EXAMPLE);
    let fake = fake(example_rules());
    let raw = case.run(&fake, &case.versions(true)).unwrap();
    let Some(BeforeOutcome::Passed(before)) = &raw.before else {
        panic!("{:?}", raw.before)
    };
    assert_eq!((before.len(), raw.head.len()), (3, 3));
    let journal = fake.journal();
    let turns = [
        "repeat-1-before",
        "repeat-1-head",
        "repeat-2-before",
        "repeat-2-head",
        "repeat-3-before",
        "repeat-3-head",
    ];
    assert_eq!(workload_folders(&case, &journal), turns);
    // Each `db` of a repeat starts from the setup snapshot of its own version.
    let made = created(&journal);
    let side_of = |launch: &Launch| {
        let folder = out_of(launch).unwrap().parent().unwrap();
        let name = folder.file_name().unwrap();
        if name.ends_with("before") {
            Side::Before
        } else {
            Side::Head
        }
    };
    let dbs = made.iter().filter(|(launch, _)| launch.service == "db");
    let mut setup = Vec::new();
    for (launch, id) in dbs {
        match &launch.seed {
            None => {
                let snapshot = journal.iter().find_map(|call| match call {
                    Call::Snapshot { id: of, snapshot } if of == id => snapshot.clone(),
                    _ => None,
                });
                setup.push((side_of(launch), snapshot.unwrap()));
            }
            Some(Seed::Snapshot(seed)) => {
                let own = setup.iter().find(|(side, _)| *side == side_of(launch));
                assert_eq!(Some(seed), own.map(|(_, snapshot)| snapshot));
            }
            other => panic!("db from {other:?}"),
        }
    }
    assert_eq!(setup.len(), 2);
    let harness: BTreeSet<Utf8PathBuf> = made
        .iter()
        .flat_map(|(launch, _)| launch.mounts.clone())
        .filter_map(|mount| match mount {
            Mount::Bind {
                host,
                guest,
                read_only: true,
            } if guest == "/harness" => Some(host),
            _ => None,
        })
        .collect();
    assert_eq!(harness.len(), 1, "one harness folder for both versions");
    assert_eq!(raw.harness, ["/harness"]);
    assert_clean(&fake, &[]);
}

#[test]
fn wall_runs_from_the_create_of_the_workload_to_its_exit() {
    // `api` gets healthy at its second attempt, so the workload starts some seconds into each
    // repeat, and each `wait` returns 250 ms after the exit it saw: `wall` is still 30 s.
    let case = Case::new(EXAMPLE);
    let api = Script {
        health: vec![Some(1), Some(0)],
        ..stops(SECOND)
    };
    let fake = fake(with("api", api));
    let sandbox = Hooked {
        latency: 250 * Duration::from_millis(1),
        ..Hooked::new(&fake)
    };
    let raw = sandbox.run(&case, &case.versions(false)).unwrap();
    let journal = fake.journal();
    let api: Vec<SandboxId> = created(&journal)
        .into_iter()
        .filter(|(launch, _)| launch.service == "api")
        .map(|(_, id)| id)
        .collect();
    let attempts = journal
        .iter()
        .filter(|call| matches!(call, Call::ExecDetached { id, .. } if api.contains(id)));
    assert_eq!(attempts.count(), 2 * api.len());
    assert_eq!(raw.head.len(), 3);
    for output in &raw.head {
        assert_eq!(output.wall, 30 * SECOND, "repeat {}", output.repeat);
    }
    assert_clean(&fake, &[]);
}

#[test]
fn a_failure_of_the_head_in_a_repeat_ends_the_run_with_what_it_collected() {
    let cases: [(&str, Script, Phase, Cause); 4] = [
        ("load", Script::default(), Phase::Workload, Cause::Timeout),
        (
            "load",
            exits(3, 10 * SECOND),
            Phase::Workload,
            Cause::Exit(3),
        ),
        (
            "api",
            Script {
                exit: Some((1, 5 * SECOND)),
                ..stops(SECOND)
            },
            Phase::Workload,
            Cause::Died,
        ),
        (
            "api",
            Script {
                health: vec![Some(1)],
                ..Script::default()
            },
            Phase::Ready,
            Cause::Unhealthy,
        ),
    ];
    for (service, script, phase, cause) in cases {
        let case = Case::new(EXAMPLE);
        let fake = fake(with(service, script));
        let failed = app_failure(case.run(&fake, &case.versions(false)));
        let failure = &failed.failure;
        assert_eq!(
            (
                failure.phase,
                failure.service.as_deref(),
                &failure.cause,
                failure.repeat
            ),
            (phase, Some(service), &cause, Some(1)),
            "{service}"
        );
        let dir = case.run_dir().join("repeat-1-head");
        assert_eq!(failure.logs.as_deref(), Some(dir.as_path()));
        assert!(dir.join(format!("{service}-1.log")).is_file());
        assert!(
            dir.join("metrics.json").is_file(),
            "a failed repeat ends whole"
        );
        let head: Vec<u32> = failed.raw.head.iter().map(|output| output.repeat).collect();
        assert_eq!(head, [1], "the failed repeat is in the raw outputs");
        assert!(!case.run_dir().join("repeat-2-head").exists());
        assert_clean(&fake, &[]);
    }

    // The workload of the second repeat never ends: the run ends with both repeats, at `timeout`
    // from the start of that repeat, the 2 s `api` takes to get healthy included.
    let case = Case::new(EXAMPLE);
    let api = Script {
        health: vec![Some(1), Some(0)],
        ..stops(SECOND)
    };
    let fake = fake(with("api", api));
    let sandbox = Hooked {
        hang: Some(("load", 2)),
        ..Hooked::new(&fake)
    };
    let result = sandbox.run(&case, &case.versions(false));
    let failed = app_failure(result);
    let failure = &failed.failure;
    assert_eq!(
        (failure.service.as_deref(), &failure.cause, failure.repeat),
        (Some("load"), &Cause::Timeout, Some(2))
    );
    let head: Vec<(u32, Duration)> = failed
        .raw
        .head
        .iter()
        .map(|output| (output.repeat, output.wall))
        .collect();
    assert_eq!(head, [(1, 30 * SECOND), (2, (300 - 2) * SECOND)]);
    let journal = fake.journal();
    let db = &created(&journal)[3 + 4].1;
    let start = index_of(
        &journal,
        |call| matches!(call, Call::Create { ids, .. } if ids.contains(db)),
    );
    let waits: Vec<Instant> = journal[start..]
        .iter()
        .filter_map(|call| match call {
            Call::Wait { deadline, .. } => Some(*deadline),
            _ => None,
        })
        .collect();
    // The first wait ends a chunk of 1 s after the start, the last one at the deadline.
    assert_eq!(*waits.iter().max().unwrap() - waits[0], 299 * SECOND);
    assert_clean(&fake, &[]);
}

#[test]
fn every_repeat_ends_with_the_stop_of_each_long_running_service_dependents_first() {
    let case = Case::new(EXAMPLE);
    let fake = fake(example_rules());
    let raw = case.run(&fake, &case.versions(false)).unwrap();
    for output in &raw.head {
        for instance in &output.instances {
            match instance.role {
                Role::LongRunning => {
                    assert!(instance.stopped.is_some_and(|stopped| !stopped.killed));
                }
                Role::Workload => assert!(instance.stopped.is_none()),
            }
        }
    }
    let journal = fake.journal();
    let made = created(&journal);
    let name = |id: &SandboxId| {
        let (launch, _) = made.iter().find(|(_, made)| made == id).unwrap();
        format!("{}-{}", launch.service, launch.replica)
    };
    let stops: Vec<(usize, String)> = journal
        .iter()
        .enumerate()
        .filter_map(|(index, call)| match call {
            Call::Stop(id) => Some((index, name(id))),
            _ => None,
        })
        .collect();
    // Setup stops `db`; each repeat the two `api` in reverse, then `db`; the workload never.
    let mut expected = vec!["db-1"];
    for _ in 0..3 {
        expected.extend(["api-2", "api-1", "db-1"]);
    }
    let names: Vec<&str> = stops.iter().map(|(_, name)| name.as_str()).collect();
    assert_eq!(names, expected);
    // In each repeat the stops come after the cut read the workload, at its exit.
    for (repeat, stops) in stops[1..].chunks(3).enumerate() {
        let load = &made[3 + 4 * repeat + 3].1;
        let cut = index_of(
            &journal,
            |call| matches!(call, Call::Metrics(id) if id == load),
        );
        assert!(cut < stops[0].0, "repeat {}", repeat + 1);
    }
    let text = fs::read_to_string(case.run_dir().join("repeat-2-head/metrics.json")).unwrap();
    let metrics: serde_json::Value = serde_json::from_str(&text).unwrap();
    let stopped: Vec<(&str, bool)> = metrics
        .as_array()
        .unwrap()
        .iter()
        .map(|instance| {
            let service = instance["service"].as_str().unwrap();
            (service, !instance["stopped"].is_null())
        })
        .collect();
    assert_eq!(
        stopped,
        [("db", true), ("api", true), ("api", true), ("load", false)]
    );
    assert_clean(&fake, &[]);
}

#[test]
fn everything_made_is_destroyed_on_every_path() {
    // Passed, the head failed in a repeat, in setup, before its workload, "before" failed in a
    // repeat.
    let paths: [fn(&Case) -> Vec<Rule>; 5] = [
        |_| example_rules(),
        |_| with("load", exits(3, SECOND)),
        |_| with("migrate", exits(1, SECOND)),
        |_| {
            let sick = Script {
                health: vec![Some(1)],
                ..Script::default()
            };
            with("api", sick)
        },
        |case| {
            let mut rules = example_rules();
            let dies = Script {
                exit: Some((1, SECOND)),
                ..stops(SECOND)
            };
            rules.insert(
                0,
                Rule {
                    service: "api".into(),
                    tree: Some(case.tar(Side::Before)),
                    script: dies,
                },
            );
            rules
        },
    ];
    for rules in paths {
        let case = Case::new(EXAMPLE);
        let fake = fake(rules(&case));
        drop(case.run(&fake, &case.versions(true)));
        assert_clean(&fake, &[]);
    }
}

fn logs_in(dir: &Utf8Path) -> BTreeSet<String> {
    let names = fs::read_dir(dir).unwrap().map(|entry| {
        let name = entry.unwrap().file_name();
        name.into_string().unwrap()
    });
    names.filter(|name| name.ends_with(".log")).collect()
}

fn names(logs: &[&str]) -> BTreeSet<String> {
    logs.iter().map(|log| format!("{log}.log")).collect()
}

#[test]
fn cancel_in_a_repeat_destroys_the_vms_and_leaves_the_raw_outputs() {
    // Ctrl+C once the workload is created is in `Workload`; once `api` is, in `Ready`.
    let cases = [
        (
            "load",
            Phase::Workload,
            &["db-1", "api-1", "api-2", "load-1"][..],
        ),
        ("api", Phase::Ready, &["db-1", "api-1", "api-2"][..]),
    ];
    for (at, phase, logs) in cases {
        let case = Case::new(EXAMPLE);
        let fake = fake(example_rules());
        let sandbox = Hooked {
            cancel_at: Some(at),
            ..Hooked::new(&fake)
        };
        let result = sandbox.run(&case, &case.versions(false));
        assert!(
            matches!(&result, Err(RunError::Cancelled { phase: own }) if *own == phase),
            "{result:?}"
        );
        let repeat = case.run_dir().join("repeat-1-head");
        assert_eq!(logs_in(&repeat), names(logs), "{at}");
        let setup = case.run_dir().join("setup-head");
        assert!(setup.join("db-1.log").is_file());
        assert!(setup.join("metrics.json").is_file());
        assert_left(&fake, &[]);
    }
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

#[test]
fn a_stand_fault_in_a_repeat_tears_it_down_in_its_phase() {
    // `Ready` before the `create` with the workload returned (its own refused `create` too),
    // `Workload` from then on.
    let cases = [
        ("postgres:17", Phase::Ready, &[][..]),
        ("redis:7", Phase::Ready, &["db-1"][..]),
        (
            "busybox:1",
            Phase::Workload,
            &["db-1", "cache-1", "load-1"][..],
        ),
    ];
    for (missing, phase, logs) in cases {
        let case = Case::new(AFTER_THE_WORKLOAD);
        let fake = Fake::new(Scenario {
            rules: vec![
                rule("db", stops(SECOND)),
                rule("cache", stops(SECOND)),
                rule("side", stops(SECOND)),
                rule("load", exits(0, 5 * SECOND)),
            ],
            missing_images: vec![missing.into()],
            ..Scenario::default()
        });
        let result = case.run(&fake, &case.versions(false));
        assert!(
            matches!(
                &result,
                Err(RunError::Stand {
                    phase: own,
                    fault: StandFault::Sandbox(SandboxError::ImageNotFound(_)),
                }) if *own == phase
            ),
            "{missing}: {result:?}"
        );
        let repeat = case.run_dir().join("repeat-1-head");
        assert_eq!(logs_in(&repeat), names(logs), "{missing}");
        assert_left(&fake, &[]);
    }

    // The stop at the end of the repeat breaks: after the workload exited, and after `db` stayed
    // unhealthy, which ends the repeat before its workload.
    let cases = [(Some(0), Phase::Workload), (Some(1), Phase::Ready)];
    for (health, phase) in cases {
        let case = Case::new(AFTER_THE_WORKLOAD);
        let db = Script {
            health: vec![health],
            ..stops(SECOND)
        };
        let fake = fake(vec![
            rule("db", db),
            rule("cache", stops(SECOND)),
            rule("side", stops(SECOND)),
            rule("load", exits(0, 5 * SECOND)),
        ]);
        let sandbox = Hooked {
            fail_stop: true,
            ..Hooked::new(&fake)
        };
        let result = sandbox.run(&case, &case.versions(false));
        assert!(
            matches!(
                &result,
                Err(RunError::Stand {
                    phase: own,
                    fault: StandFault::Sandbox(SandboxError::Backend { .. }),
                }) if *own == phase
            ),
            "{health:?}: {result:?}"
        );
        assert!(case.run_dir().join("repeat-1-head/db-1.log").is_file());
        assert_left(&fake, &[]);
    }
}

// --- probes and the cut ------------------------------------------------------------------------

/// `EXAMPLE` with an `on-failure` probe in `api`, which no setup starts.
fn traced() -> String {
    EXAMPLE.replace(
        "  probes:\n",
        "  probes:\n    trace: { in: api, at: on-failure, run: [\"cat\", \"/tmp/trace\"] }\n",
    )
}

/// Each `Exec` of `journal`: its index, its `stdout` and its VM as `service-replica`.
fn execs(journal: &[Call]) -> Vec<(usize, Utf8PathBuf, String)> {
    let made = created(journal);
    let mut execs = Vec::new();
    for (index, call) in journal.iter().enumerate() {
        if let Call::Exec { id, stdout, .. } = call {
            let (launch, _) = made.iter().find(|(_, made)| made == id).unwrap();
            let vm = format!("{}-{}", launch.service, launch.replica);
            execs.push((index, stdout.clone(), vm));
        }
    }
    execs
}

fn ran(probes: &[ProbeRan]) -> Vec<&str> {
    probes.iter().map(|probe| probe.name.as_str()).collect()
}

/// No `Stop` from the last `Create` before `index` to `index`: a probe there comes before the
/// stops of its phase.
fn before_the_stops(journal: &[Call], index: usize) -> bool {
    let phase = journal[..index]
        .iter()
        .rposition(|call| matches!(call, Call::Create { .. }));
    let phase = &journal[phase.unwrap()..index];
    !phase.iter().any(|call| matches!(call, Call::Stop(_)))
}

#[test]
fn probes_run_at_their_taps_and_write_into_out() {
    let case = Case::new(EXAMPLE);
    let fake = fake(example_rules());
    let sandbox = Hooked::new(&fake);
    let raw = sandbox.run(&case, &case.versions(true)).unwrap();
    let setups = [
        (Side::Head, &raw.setup.head),
        (Side::Before, raw.setup.before.as_ref().unwrap()),
    ];
    for (side, setup) in setups {
        assert_eq!(ran(&setup.probes), ["schema"], "{side}");
        let schema = &setup.probes[0];
        let out = case.run_dir().join(format!("setup-{side}/out"));
        assert_eq!(schema.out, out.join("probe.schema.jsonl"));
        assert_eq!(schema.outcome, ExecOutcome::Exited(0));
        assert_eq!(fs::read(&schema.out).unwrap(), b"{\"table\":\"orders\"}\n");
        assert!(
            out.join("probe.schema.jsonl.err").is_file(),
            "its stderr beside it"
        );
    }
    let Some(BeforeOutcome::Passed(before)) = &raw.before else {
        panic!("{:?}", raw.before)
    };
    for output in raw.head.iter().chain(before) {
        assert_eq!(ran(&output.probes), ["orders"], "repeat {}", output.repeat);
        let orders = &output.probes[0];
        assert_eq!(orders.out, output.out_dir.join("probe.orders.jsonl"));
        assert_eq!(fs::read(&orders.out).unwrap(), b"{\"id\":1}\n");
        assert!(
            output.out_dir.join("orders.jsonl").is_file(),
            "the workload's file"
        );
    }
    let files: Vec<&str> = raw.probes.keys().map(String::as_str).collect();
    assert_eq!(
        files,
        [
            "probe.last.jsonl",
            "probe.orders.jsonl",
            "probe.schema.jsonl"
        ]
    );
    let orders = &raw.probes["probe.orders.jsonl"];
    assert_eq!(
        (orders.key.as_deref(), orders.ignore.as_slice()),
        (Some("/id"), ["/at".to_owned()].as_slice())
    );
    // Each in replica 1 of `db` with its own `run` and 30 s to go, before the stops of its phase:
    // `schema` once per setup, `orders` in each repeat, `last` never.
    let journal = fake.journal();
    let mut phases = Vec::new();
    for (index, stdout, vm) in execs(&journal) {
        assert_eq!(vm, "db-1");
        assert!(before_the_stops(&journal, index), "{stdout}");
        let Call::Exec { argv, .. } = &journal[index] else {
            unreachable!()
        };
        let phase = stdout.parent().unwrap().parent().unwrap().file_name();
        phases.push((phase.unwrap().to_owned(), argv[3].as_str()));
    }
    let mut expected = vec![
        ("setup-before".to_owned(), "select 1"),
        ("setup-head".to_owned(), "select 1"),
    ];
    for repeat in 1..=3 {
        for side in ["before", "head"] {
            expected.push((format!("repeat-{repeat}-{side}"), "select 2"));
        }
    }
    assert_eq!(phases, expected);
    assert_eq!(*sandbox.deadlines.borrow(), [30 * SECOND; 2 + 2 * 3]);
    assert_clean(&fake, &[]);
}

#[test]
fn the_cut_reads_every_instance_before_the_first_probe() {
    let case = Case::new(EXAMPLE);
    let fake = fake(example_rules());
    let sandbox = Hooked {
        logs_grow: true,
        ..Hooked::new(&fake)
    };
    let raw = sandbox.run(&case, &case.versions(false)).unwrap();
    let journal = fake.journal();
    let made = created(&journal);
    let probes: Vec<usize> = execs(&journal).iter().map(|(index, ..)| *index).collect();
    let mut cuts = Vec::new();
    for probe in probes {
        let before = &journal[..probe];
        // The measured window ends with the last `wait`: the exit of the workload, or of the last
        // setup service.
        let window = before
            .iter()
            .rposition(|call| matches!(call, Call::Wait { .. }));
        let cut = &before[window.unwrap()..];
        let mut read = Vec::new();
        for (launch, id) in &made {
            let live = before
                .iter()
                .any(|call| matches!(call, Call::Create { ids, .. } if ids.contains(id)))
                && !before
                    .iter()
                    .any(|call| matches!(call, Call::Destroy(gone) if gone == id));
            if !live {
                continue;
            }
            let metrics = cut
                .iter()
                .any(|call| matches!(call, Call::Metrics(of) if of == id));
            let logs = cut
                .iter()
                .any(|call| matches!(call, Call::Logs(of) if of == id));
            assert!(metrics && logs, "{}-{}", launch.service, launch.replica);
            read.push(format!("{}-{}", launch.service, launch.replica));
        }
        cuts.push(read);
    }
    let repeat = ["db-1", "api-1", "api-2", "load-1"];
    assert_eq!(
        cuts,
        [
            &["db-1", "prepare-1", "migrate-1"][..],
            &repeat,
            &repeat,
            &repeat
        ]
    );
    // Each read of a log is a line longer than the one before: the cut read one line, the drain
    // wrote it with what came after.
    let first_line = |log: &Utf8Path| {
        let bytes = fs::read(log).unwrap();
        let line = bytes.iter().position(|&byte| byte == b'\n').unwrap() + 1;
        assert!(bytes.len() > line, "{log} has what came after the cut");
        line as u64
    };
    for output in &raw.head {
        let dir = output.out_dir.parent().unwrap();
        let text = fs::read_to_string(dir.join("metrics.json")).unwrap();
        let metrics: serde_json::Value = serde_json::from_str(&text).unwrap();
        let written = metrics.as_array().unwrap();
        for (instance, written) in output.instances.iter().zip(written) {
            assert_eq!(instance.log_cut, first_line(&instance.log), "{dir}");
            assert_eq!(written["logCut"], instance.log_cut);
        }
    }
    let setup = case.run_dir().join("setup-head");
    let text = fs::read_to_string(setup.join("metrics.json")).unwrap();
    let metrics: serde_json::Value = serde_json::from_str(&text).unwrap();
    for written in metrics.as_array().unwrap() {
        let vm = format!(
            "{}-{}",
            written["service"].as_str().unwrap(),
            written["replica"]
        );
        let log = setup.join(format!("{vm}.log"));
        assert_eq!(written["logCut"], first_line(&log), "{vm}");
    }
    assert_clean(&fake, &[]);
}

#[test]
fn on_failure_probes_run_before_the_stop_and_their_outputs_are_in_the_failure() {
    // The workload fails in the first repeat: `last` in `db` and `trace` in replica 1 of `api`
    // run there instead of `orders`.
    let case = Case::new(&traced());
    let failing = fake(with("load", exits(4, SECOND)));
    let failed = app_failure(case.run(&failing, &case.versions(false)));
    let out = case.run_dir().join("repeat-1-head/out");
    let outputs = [out.join("probe.last.jsonl"), out.join("probe.trace.jsonl")];
    assert_eq!(failed.failure.probes, outputs);
    assert_eq!(ran(&failed.raw.head[0].probes), ["last", "trace"]);
    let journal = failing.journal();
    let probed = execs(&journal);
    let vms: Vec<(&str, &str)> = probed
        .iter()
        .map(|(_, stdout, vm)| (stdout.file_name().unwrap(), vm.as_str()))
        .collect();
    assert_eq!(
        vms,
        [
            ("probe.schema.jsonl", "db-1"),
            ("probe.last.jsonl", "db-1"),
            ("probe.trace.jsonl", "api-1")
        ]
    );
    for (index, stdout, _) in &probed {
        assert!(before_the_stops(&journal, *index), "{stdout}");
    }
    assert_clean(&failing, &[]);

    // `migrate` fails in setup: `last` runs, `trace` does not, since no setup starts `api`.
    let case = Case::new(&traced());
    let failing = fake(with("migrate", exits(1, 2 * SECOND)));
    let failed = app_failure(case.run(&failing, &case.versions(false)));
    let out = case.run_dir().join("setup-head/out/probe.last.jsonl");
    assert_eq!(failed.failure.probes, [out]);
    assert_eq!(ran(&failed.raw.setup.head.probes), ["last"]);
    let journal = failing.journal();
    let probed = execs(&journal);
    assert_eq!(probed.len(), 1);
    let (index, _, vm) = &probed[0];
    assert_eq!(vm, "db-1");
    assert!(before_the_stops(&journal, *index));
    assert_clean(&failing, &[]);
}

#[test]
fn a_probe_that_times_out_or_exits_nonzero_is_what_it_saw() {
    let case = Case::new(EXAMPLE);
    let db = Script {
        volume_bytes: 4096,
        probes: vec![
            (
                "probe.schema.jsonl".into(),
                ExecOutcome::Exited(2),
                b"half\n".to_vec(),
            ),
            (
                "probe.orders.jsonl".into(),
                ExecOutcome::TimedOut,
                Vec::new(),
            ),
            ("probe.last.jsonl".into(), ExecOutcome::TimedOut, Vec::new()),
        ],
        ..stops(SECOND)
    };
    let saw = fake(with("db", db.clone()));
    let raw = case.run(&saw, &case.versions(false)).unwrap();
    let schema = &raw.setup.head.probes[0];
    assert_eq!(schema.outcome, ExecOutcome::Exited(2));
    assert_eq!(fs::read(&schema.out).unwrap(), b"half\n");
    assert_eq!(raw.head.len(), 3, "the run goes on");
    for output in &raw.head {
        assert_eq!(output.probes[0].outcome, ExecOutcome::TimedOut);
        assert!(output.probes[0].out.is_file());
    }
    assert_clean(&saw, &[]);

    // An `on-failure` probe past its deadline is still an output of the failure.
    let case = Case::new(EXAMPLE);
    let mut rules = with("db", db);
    rules.retain(|rule| rule.service != "load");
    rules.push(rule("load", exits(4, SECOND)));
    let saw = fake(rules);
    let failed = app_failure(case.run(&saw, &case.versions(false)));
    let last = case.run_dir().join("repeat-1-head/out/probe.last.jsonl");
    assert_eq!(failed.failure.probes, [last]);
    assert_eq!(failed.raw.head[0].probes[0].outcome, ExecOutcome::TimedOut);
    assert_clean(&saw, &[]);
}

#[test]
fn stand_out_goes_to_the_workload_and_every_service_with_a_probe() {
    let cases = [
        (EXAMPLE.to_owned(), &["db", "load"][..]),
        (traced(), &["api", "db", "load"][..]),
    ];
    for (recipe, probed) in cases {
        let case = Case::new(&recipe);
        let fake = fake(example_rules());
        case.run(&fake, &case.versions(true)).unwrap();
        let mut bound = BTreeSet::new();
        for (launch, _) in created(&fake.journal()) {
            let outs: Vec<(&Utf8Path, bool)> = launch
                .mounts
                .iter()
                .filter_map(|mount| match mount {
                    Mount::Bind {
                        host,
                        guest,
                        read_only,
                    } if guest == "/stand/out" => Some((host.as_path(), *read_only)),
                    _ => None,
                })
                .collect();
            match outs.as_slice() {
                [] => {}
                // The `out/` of its phase, writable.
                [(host, false)] => {
                    assert_eq!(host.file_name(), Some("out"));
                    let phase = host.parent().unwrap();
                    assert_eq!(phase.parent(), Some(case.run_dir().as_path()));
                    bound.insert(launch.service.as_str().to_owned());
                }
                other => panic!("{}: {other:?}", launch.service),
            }
        }
        let probed: BTreeSet<String> = probed.iter().map(|name| name.to_string()).collect();
        assert_eq!(bound, probed);
        assert_clean(&fake, &[]);
    }
}
