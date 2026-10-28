//! `run` on a `Fake` with its virtual clock. Each case loads a recipe with `spec::load` (only
//! `spec` builds its newtypes), runs it, and reads the journal of the `Fake`.

use std::collections::BTreeSet;
use std::fs;
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use report::{BeforeOutcome, Cause, Phase, Raw};
use run::{Cancel, RunError, SetupCache, Side, StandFault, Version, Versions};
use sandbox::{
    Call, ExecId, ExecOutcome, Exited, Fake, Fault, ImageRef, Launch, Leftovers, Metrics, Pulled,
    Rule, Sandbox, SandboxError, SandboxId, Scenario, Script, Seed, SnapshotId, Stopped,
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

/// The scripts of `EXAMPLE` where everything passes: `prepare` takes 20 s, `migrate` 2 s, `load` 30 s.
fn example_rules() -> Vec<Rule> {
    vec![
        rule(
            "db",
            Script {
                volume_bytes: 4096,
                ..stops(300 * Duration::from_millis(1))
            },
        ),
        rule("prepare", exits(0, 20 * SECOND)),
        rule("migrate", exits(0, 2 * SECOND)),
        rule("api", stops(100 * Duration::from_millis(1))),
        rule("load", exits(0, 30 * SECOND)),
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
        assert_eq!(seeded, BTreeSet::from(["migrate"]), "{side}");
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
    // `api` runs in setup, for `fill`; no setup service mounts the tree.
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
    assert_eq!(api.len(), 1);
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
}

/// The `Fake` with what a script cannot say: `cancel` set once a `create` starts `cancel_at`
/// (Ctrl+C in the middle of a phase).
struct Hooked<'f> {
    fake: &'f Fake,
    cancel_at: Option<&'static str>,
    cancel: Cancel,
}

impl<'f> Hooked<'f> {
    fn new(fake: &'f Fake) -> Hooked<'f> {
        Hooked {
            fake,
            cancel_at: None,
            cancel: Cancel::new(),
        }
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
        if batch
            .iter()
            .any(|launch| self.cancel_at == Some(launch.service.as_str()))
        {
            self.cancel.cancel();
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
        self.fake.wait(what, deadline)
    }
    fn exec(
        &self,
        id: &SandboxId,
        argv: &[String],
        deadline: Instant,
        stdout: &Utf8Path,
    ) -> Result<ExecOutcome, SandboxError> {
        self.fake.exec(id, argv, deadline, stdout)
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
    let result = run::run(
        &case.stand,
        &sandbox,
        &case.versions(true),
        &case.run_dir(),
        &sandbox.cancel,
    );
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
    assert_clean(&fake, &[]);
}
