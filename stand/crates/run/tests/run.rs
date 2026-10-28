//! `run` and `inspect` on a `Fake` with its virtual clock. Each case loads a recipe with
//! `spec::load` (only `spec` builds its newtypes), runs it, and reads the journal of the `Fake`.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use profile::ProfileError;
use report::{
    BeforeOutcome, Cause, CoverageOutcome, Extra, Failure, Phase, ProbeRan, ProfileOutcome, Raw,
    Role,
};
use run::{Cancel, InspectError, RunError, SetupCache, Side, StandFault, Version, Versions};
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
        let version = snapshot_of(&journal, prepare);
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
        let coverage = usize::from(side == Side::Head);
        assert_eq!(
            api.count(),
            2 * (3 + coverage),
            "replicas x repeats, and the coverage repeat of the head"
        );
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
    assert_clean(&fake, &case.left());
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

#[test]
fn without_a_prepare_service_every_tree_service_unpacks_the_tree() {
    let case = Case::new(NO_PREPARE);
    assert!(case.stand.prepare.is_none());
    let fake = fake(no_prepare_rules());
    case.run(&fake, &case.versions(false)).unwrap();
    let journal = fake.journal();
    let api: Vec<_> = created(&journal)
        .into_iter()
        .filter(|(launch, _)| launch.service == "api")
        .collect();
    assert_eq!(
        api.len(),
        1 + 2 + 1,
        "setup, the repeats, the coverage repeat"
    );
    for (launch, _) in api {
        assert!(matches!(launch.seed, Some(Seed::Tree { tar, .. }) if tar == case.tar(Side::Head)));
    }
    assert!(
        !journal
            .iter()
            .any(|call| matches!(call, Call::Snapshot { .. }))
    );
    assert_clean(&fake, &case.left());
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
    assert_eq!(
        snapshots, 2,
        "db in setup, and at the end of the last repeat"
    );
    // Only the long-running `db` is stopped; the setup services have exited.
    let metrics = metrics_in(&case.run_dir().join("setup-head"));
    assert_eq!(
        stopped(&metrics),
        [("db", true), ("prepare", false), ("migrate", false)]
    );
    assert!(case.run_dir().join("setup-head/db-1.log").is_file());
    assert_clean(&fake, &case.left());
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
    assert!(!case.run_dir().join("kept.json").exists());
    assert!(failed.raw.head.is_empty());
    let entry = case.entry(Side::Head);
    assert_eq!(run::cached_ids(&entry).unwrap(), None);
    assert!(names_in(&entry).is_empty(), "nothing in the entry");
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
    assert_clean(&fake, &case.left());
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
    assert_clean(&hung, &case.left());

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
    assert_clean(&sick, &case.left());

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
    assert_clean(&hanging, &case.left());

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
    assert_clean(&cut, &case.left());

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
    assert_clean(&starting, &case.left());
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
    assert_clean(&fake, &case.left());

    // `db` never gets healthy and `prepare` crashes meanwhile: the crash ends setup, not the deadline.
    let case = Case::new(EXAMPLE);
    let db = Script {
        health: vec![Some(1)],
        ..Script::default()
    };
    let rules = replaced(with("db", db), "prepare", exits(2, SECOND));
    let crashing = crate::fake(rules);
    let failure = app_failure(case.run(&crashing, &case.versions(false))).failure;
    assert_eq!(
        (failure.service.as_deref(), failure.cause),
        (Some("prepare"), Cause::Exit(2))
    );
    assert_clean(&crashing, &case.left());
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
    assert_clean(&fake, &case.left());
}

/// The `Fake` with what a script cannot say: `cancel` set once a `create` starts `cancel_at`
/// (Ctrl+C in the middle of a phase); `hang`: the `n`-th VM of a service, from 1, never ends,
/// its exit kept out of every `wait`; `fail_stop`: every `stop` is a `Backend` error;
/// `logs_grow`: each `logs` of a VM is one line longer than the one before, as the output of a
/// running service grows; `latency`: a `wait` that saw an exit returns this much after it, as
/// the adapter's poll does, while `Exited.at` keeps when it ended; `entry`: a file whose
/// presence each `create` and `destroy` notes in `seen`, by VM.
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
    entry: Option<Utf8PathBuf>,
    seen: RefCell<Vec<(SandboxId, bool)>>,
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
    fn create(&self, batch: &[Launch]) -> Result<Vec<SandboxId>, SandboxError> {
        let ids = self.fake.create(batch)?;
        for (launch, id) in batch.iter().zip(&ids) {
            self.note(id);
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
    assert_left(&fault, &case.left());

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
    assert_left(&fake, &case.left());

    // `preload` in the run folder is a file: the preload of `api` cannot be written, and nothing
    // starts.
    let case = Case::new(EXAMPLE);
    let preload = case.run_dir().join("preload");
    fs::write(&preload, b"").unwrap();
    let blocked = crate::fake(example_rules());
    let result = case.run(&blocked, &case.versions(false));
    assert!(
        matches!(
            &result,
            Err(RunError::Stand {
                phase: Phase::Setup,
                fault: StandFault::Io { path, .. },
            }) if *path == preload
        ),
        "{result:?}"
    );
    assert!(blocked.journal().is_empty());
}

#[test]
fn a_failure_of_before_stops_only_its_lane() {
    let case = Case::new(EXAMPLE);
    // `migrate` of the before tree fails; the head's passes.
    let fake = fake(on_before(
        &case,
        example_rules(),
        "migrate",
        exits(1, SECOND),
    ));
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
    let version = snapshot_of(&journal, &prepare(Side::Before));
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
    // Only the setup that passed has an entry.
    assert!(names_in(&case.entry(Side::Before)).is_empty());
    let head = run::cached_ids(&case.entry(Side::Head)).unwrap();
    assert_eq!(head.map(|ids| ids.len()), Some(2));
    assert_clean(&fake, &case.left());

    // `api` of the before tree dies in its first repeat: "before" runs no other repeat.
    let case = Case::new(EXAMPLE);
    let dies = Script {
        exit: Some((1, 5 * SECOND)),
        ..stops(SECOND)
    };
    let rules = on_before(&case, example_rules(), "api", dies);
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
        "coverage",
    ];
    assert_eq!(workload_folders(&case, &dying.journal()), repeats);
    assert!(case.run_dir().join("repeat-1-before/api-1.log").is_file());
    assert_clean(&dying, &case.left());
}

// --- the measured repeats ----------------------------------------------------------------------

/// The host folder of the `/stand/out` bind of `launch`.
fn out_of(launch: &Launch) -> Option<&Utf8Path> {
    launch.mounts.iter().find_map(|mount| match mount {
        Mount::Bind { host, guest, .. } if guest == "/stand/out" => Some(host.as_path()),
        _ => None,
    })
}

/// The folder of the phase whose `/stand/out` `launch` binds: `setup-head`, `repeat-2-before`.
fn phase_of(launch: &Launch) -> &str {
    let out = out_of(launch).expect("a bind of /stand/out");
    out.parent().unwrap().file_name().unwrap()
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

/// From the first `wait` after the `create` of `id` to the deadline of the last one.
fn waited(journal: &[Call], id: &SandboxId) -> Duration {
    let start = index_of(
        journal,
        |call| matches!(call, Call::Create { ids, .. } if ids.contains(id)),
    );
    let waits: Vec<Instant> = journal[start..]
        .iter()
        .filter_map(|call| match call {
            Call::Wait { deadline, .. } => Some(*deadline),
            _ => None,
        })
        .collect();
    *waits.iter().max().unwrap() - waits[0]
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
    // Setup, then each repeat by levels: `db`, the two `api`, `load`; setup services once. The
    // coverage repeat comes last.
    let repeat = ["db-1", "api-1", "api-2", "load-1"];
    let mut expected = vec!["db-1", "prepare-1", "migrate-1"];
    for _ in 0..3 + 1 {
        expected.extend(repeat);
    }
    assert_eq!(services(&journal), expected);
    let made = created(&journal);
    let db_snapshot = snapshot_of(&journal, &made[0].1);
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
    assert_clean(&fake, &case.left());
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
        "coverage",
    ];
    assert_eq!(workload_folders(&case, &journal), turns);
    // Each `db` of a repeat starts from the setup snapshot of its own version.
    let made = created(&journal);
    let side_of = |launch: &Launch| {
        if phase_of(launch).ends_with("before") {
            Side::Before
        } else {
            Side::Head
        }
    };
    let dbs = made.iter().filter(|(launch, _)| launch.service == "db");
    let mut setup = Vec::new();
    for (launch, id) in dbs {
        match &launch.seed {
            None => setup.push((side_of(launch), snapshot_of(&journal, id))),
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
    assert_clean(&fake, &case.left());
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
    assert_clean(&fake, &case.left());
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
        assert_clean(&fake, &case.left());
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
    // The first wait ends a chunk of 1 s after the start, the last one at the deadline.
    assert_eq!(waited(&journal, db), 299 * SECOND);
    assert_clean(&fake, &case.left());
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
        vm(launch)
    };
    let stops: Vec<(usize, String)> = journal
        .iter()
        .enumerate()
        .filter_map(|(index, call)| match call {
            Call::Stop(id) => Some((index, name(id))),
            _ => None,
        })
        .collect();
    // Setup stops `db`; each repeat, the coverage repeat too, the two `api` in reverse, then `db`;
    // the workload never.
    let mut expected = vec!["db-1"];
    for _ in 0..3 + 1 {
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
    let metrics = metrics_in(&case.run_dir().join("repeat-2-head"));
    assert_eq!(
        stopped(&metrics),
        [("db", true), ("api", true), ("api", true), ("load", false)]
    );
    assert_clean(&fake, &case.left());
}

#[test]
fn everything_made_is_destroyed_on_every_path() {
    // Passed, the head failed in a repeat, in setup, before its workload, "before" failed in a
    // repeat. A failure of both versions fails "before" first.
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
            let dies = Script {
                exit: Some((1, SECOND)),
                ..stops(SECOND)
            };
            on_before(case, example_rules(), "api", dies)
        },
    ];
    // What each keeps, by the folder of the repeat the snapshot came from.
    let last = [
        ["before", "db", "repeat-3-before"],
        ["head", "db", "repeat-3-head"],
    ];
    let first = [
        ["before", "db", "repeat-1-before"],
        ["head", "db", "repeat-1-head"],
    ];
    let before_failed = [
        ["before", "db", "repeat-1-before"],
        ["head", "db", "repeat-3-head"],
    ];
    let kept: [&[[&str; 3]]; 5] = [&last, &first, &[], &first, &before_failed];
    for (rules, kept) in paths.into_iter().zip(kept) {
        let case = Case::new(EXAMPLE);
        let fake = fake(rules(&case));
        drop(case.run(&fake, &case.versions(true)));
        assert_eq!(kept_from(&case, &fake.journal()), kept);
        assert_clean(&fake, &case.left());
    }
}

fn logs_in(dir: &Utf8Path) -> BTreeSet<String> {
    let names = names_in(dir).into_iter();
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
        assert_left(&fake, &case.left());
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
        assert_left(&fake, &case.left());
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
        assert_left(&fake, &case.left());
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
            execs.push((index, stdout.clone(), vm(launch)));
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
    assert_clean(&fake, &case.left());
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
            assert!(metrics && logs, "{}", vm(launch));
            read.push(vm(launch));
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
        for (instance, written) in output.instances.iter().zip(metrics_in(dir)) {
            assert_eq!(instance.log_cut, first_line(&instance.log), "{dir}");
            assert_eq!(written["logCut"], instance.log_cut);
        }
    }
    let setup = case.run_dir().join("setup-head");
    for written in metrics_in(&setup) {
        let vm = format!(
            "{}-{}",
            written["service"].as_str().unwrap(),
            written["replica"]
        );
        let log = setup.join(format!("{vm}.log"));
        assert_eq!(written["logCut"], first_line(&log), "{vm}");
    }
    assert_clean(&fake, &case.left());
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
    assert_clean(&failing, &case.left());

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
    assert_clean(&failing, &case.left());
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
    assert_clean(&saw, &case.left());

    // An `on-failure` probe past its deadline is still an output of the failure.
    let case = Case::new(EXAMPLE);
    let saw = fake(replaced(with("db", db), "load", exits(4, SECOND)));
    let failed = app_failure(case.run(&saw, &case.versions(false)));
    let last = case.run_dir().join("repeat-1-head/out/probe.last.jsonl");
    assert_eq!(failed.failure.probes, [last]);
    assert_eq!(failed.raw.head[0].probes[0].outcome, ExecOutcome::TimedOut);
    assert_clean(&saw, &case.left());
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
        assert_clean(&fake, &case.left());
    }
}

// --- profiles and the coverage repeat ----------------------------------------------------------

/// One chunk of a Node process: `order` at 5:16 of `dist/main.js` in the tree, three samples of
/// 1 ms.
const CPU_PROFILE: &str = r#"{"nodes":[{"id":1,"callFrame":{"functionName":"(root)","scriptId":"0","url":"","lineNumber":-1,"columnNumber":-1},"children":[2]},{"id":2,"callFrame":{"functionName":"order","scriptId":"1","url":"file:///w/app/dist/main.js","lineNumber":4,"columnNumber":15}}],"startTime":0,"endTime":4000,"samples":[2,2,2],"timeDeltas":[1000,1000,1000]}"#;

/// One coverage chunk of the same process: three calls of `order`.
const COVERAGE_CHUNK: &str = r#"[{"url":"file:///w/app/dist/main.js","functions":[{"functionName":"order","ranges":[{"count":3}],"line":4,"column":15}]}]"#;

/// `rules` with an `api` whose one Node process leaves `files` in `/stand/profile`.
fn chunks(rules: Vec<Rule>, files: &[(&str, &str)]) -> Vec<Rule> {
    let mut writes = vec![("/stand/profile/1.start.json".to_owned(), b"{}".to_vec())];
    for (name, text) in files {
        writes.push((format!("/stand/profile/{name}"), text.as_bytes().to_vec()));
    }
    let api = Script {
        writes,
        ..stops(100 * Duration::from_millis(1))
    };
    replaced(rules, "api", api)
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

#[test]
fn a_profiled_service_gets_the_preload_its_chunks_folder_and_its_node_options_after_the_preload() {
    let case = Case::new(EXAMPLE);
    let fake = fake(example_rules());
    case.run(&fake, &case.versions(true)).unwrap();
    let preload = case.run_dir().join("preload");
    assert_eq!(
        fs::read_to_string(preload.join("preload.cjs")).unwrap(),
        profile::PRELOAD
    );
    let mut modes = Vec::new();
    for (launch, _) in created(&fake.journal()) {
        if launch.service != "api" {
            assert_eq!(profile_mounts(&launch), (None, false), "{}", launch.service);
            assert!(
                !launch.env.contains_key("NODE_OPTIONS"),
                "{}",
                launch.service
            );
            continue;
        }
        assert_eq!(profile_mounts(&launch), (Some(preload.as_path()), true));
        assert_eq!(
            launch.env["NODE_OPTIONS"],
            r#"--require "/stand/preload/preload.cjs" --max-old-space-size=512"#
        );
        assert_eq!(launch.env["STOP_SLOP_PROFILE_DIR"], "/stand/profile");
        modes.push(launch.env["STOP_SLOP_PROFILE_MODE"].clone());
    }
    // Both replicas in each of the six measured repeats, then in the coverage repeat.
    let mut expected = vec!["cpu"; 2 * 6];
    expected.extend(["coverage"; 2]);
    assert_eq!(modes, expected);
    assert_clean(&fake, &case.left());

    // `api` gets no preload in setup, and the setup service `fill` never does.
    let case = Case::new(NO_PREPARE);
    let fake = crate::fake(no_prepare_rules());
    case.run(&fake, &case.versions(false)).unwrap();
    let preload = case.run_dir().join("preload");
    let mut profiled = Vec::new();
    for (launch, _) in created(&fake.journal()) {
        let (bound, scratch) = profile_mounts(&launch);
        assert_eq!(
            bound,
            scratch.then_some(preload.as_path()),
            "{}",
            launch.service
        );
        if scratch {
            let options = &launch.env["NODE_OPTIONS"];
            assert_eq!(options, r#"--require "/stand/preload/preload.cjs""#);
        }
        profiled.push((launch.service, scratch));
    }
    let expected = [
        ("api", false),
        ("fill", false),
        ("api", true),
        ("load", false),
        ("api", true),
        ("load", false),
        ("api", true),
        ("load", false),
    ];
    let expected: Vec<(String, bool)> = expected
        .iter()
        .map(|(service, scratch)| (service.to_string(), *scratch))
        .collect();
    assert_eq!(profiled, expected);
    assert_clean(&fake, &case.left());
}

#[test]
fn the_chunks_come_out_before_destroy_and_an_empty_folder_is_no_profile() {
    // `api` leaves no chunk: each of its folders comes out empty.
    let case = Case::new(EXAMPLE);
    let fake = fake(example_rules());
    let raw = case.run(&fake, &case.versions(false)).unwrap();
    let journal = fake.journal();
    let mut hosts = Vec::new();
    for (index, call) in journal.iter().enumerate() {
        let Call::CopyOut { id, guest, host } = call else {
            continue;
        };
        assert_eq!(guest, "/stand/profile");
        let stopped = index_of(
            &journal,
            |call| matches!(call, Call::Stop(made) if made == id),
        );
        let gone = index_of(
            &journal,
            |call| matches!(call, Call::Destroy(made) if made == id),
        );
        assert!(stopped < index && index < gone, "{host}");
        assert!(host.is_dir(), "{host}");
        hosts.push(host.clone());
    }
    let mut expected = Vec::new();
    for phase in [
        "repeat-1-head",
        "repeat-2-head",
        "repeat-3-head",
        "coverage",
    ] {
        for replica in [1, 2] {
            let folder = case.run_dir().join(phase).join("profile");
            expected.push(folder.join(format!("api-{replica}")));
        }
    }
    assert_eq!(hosts, expected);
    for output in &raw.head {
        for instance in &output.instances {
            match instance.service.as_str() {
                "api" => assert!(matches!(instance.profile, Some(ProfileOutcome::NoProfile))),
                _ => assert!(instance.profile.is_none(), "{}", instance.service),
            }
        }
    }
    assert_clean(&fake, &case.left());

    // A chunk comes out and is read on the host, with paths from the repo root; a failed repeat
    // brings its chunks out too.
    let cases = [example_rules(), with("load", exits(3, SECOND))];
    for rules in cases {
        let case = Case::new(EXAMPLE);
        let fake = crate::fake(chunks(rules, &[("1.0.cpuprofile", CPU_PROFILE)]));
        let head = match case.run(&fake, &case.versions(false)) {
            Ok(raw) => raw.head,
            Err(RunError::App(failed)) => failed.raw.head,
            Err(other) => panic!("{other:?}"),
        };
        for output in &head {
            let api = output
                .instances
                .iter()
                .filter(|instance| instance.service == "api");
            for instance in api {
                let Some(ProfileOutcome::Read(samples)) = &instance.profile else {
                    panic!("api-{}: {:?}", instance.replica, instance.profile)
                };
                let functions: Vec<(&str, u32, u32, &str, f64)> = samples
                    .functions
                    .iter()
                    .map(|f| {
                        (
                            f.file.as_str(),
                            f.line,
                            f.column,
                            f.name.as_str(),
                            f.self_ms,
                        )
                    })
                    .collect();
                assert_eq!(functions, [("dist/main.js", 5, 16, "order", 3.0)]);
                assert_eq!(samples.processes, 1);
            }
        }
        assert_clean(&fake, &case.left());
    }

    // A chunk the host cannot read breaks the stand: in the first measured repeat, and, as a
    // coverage chunk, which a CPU read leaves alone, in the coverage repeat.
    let cases = [
        ("1.0.cpuprofile", "repeat-1-head"),
        ("1.0.coverage.json", "coverage"),
    ];
    for (file, phase) in cases {
        let case = Case::new(EXAMPLE);
        let fake = crate::fake(chunks(example_rules(), &[(file, "{")]));
        let result = case.run(&fake, &case.versions(false));
        assert!(
            matches!(
                &result,
                Err(RunError::Stand {
                    phase: Phase::Workload,
                    fault: StandFault::Profile(ProfileError::BadProfile { path, .. }),
                }) if path.starts_with(case.run_dir().join(phase))
            ),
            "{file}: {result:?}"
        );
        // `kept.json` on this path too: the last measured repeat kept `db`.
        let kept = usize::from(phase == "coverage");
        assert_eq!(case.kept().len(), kept, "{file}");
        assert_clean(&fake, &case.left());
    }
}

#[test]
fn the_coverage_repeat_runs_once_for_the_head_with_its_longer_timeout() {
    let case = Case::new(EXAMPLE);
    let fake = fake(chunks(
        example_rules(),
        &[("1.0.coverage.json", COVERAGE_CHUNK)],
    ));
    let raw = case.run(&fake, &case.versions(true)).unwrap();
    assert_eq!(
        raw.head.len(),
        3,
        "the coverage repeat is not a measured one"
    );
    let Some(BeforeOutcome::Passed(before)) = &raw.before else {
        panic!("{:?}", raw.before)
    };
    assert_eq!(before.len(), 3);
    // The calls of the two replicas add up.
    let Some(CoverageOutcome::Read(calls)) = &raw.coverage else {
        panic!("{:?}", raw.coverage)
    };
    let mut called = Vec::new();
    for (service, coverage) in calls {
        for c in &coverage.called {
            let at = (c.file.as_str(), c.line, c.column, c.name.as_str());
            called.push((service.as_str(), at, c.calls));
        }
    }
    assert_eq!(called, [("api", ("dist/main.js", 5, 16, "order"), 6)]);
    // Last, once, from the snapshots of the head: its setup snapshot of `db`, its version snapshot.
    let journal = fake.journal();
    let made = created(&journal);
    let coverage = case.run_dir().join("coverage");
    let last = &made[made.len() - 4..];
    let names: Vec<String> = last.iter().map(|(launch, _)| vm(launch)).collect();
    assert_eq!(names, ["db-1", "api-1", "api-2", "load-1"]);
    assert_eq!(out_of(&last[3].0), Some(coverage.join("out").as_path()));
    let (db, version) = setup_snapshots(&case, &journal, Side::Head);
    for ((launch, _), seed) in last[..3].iter().zip([&db, &version, &version]) {
        let restores = matches!(&launch.seed, Some(Seed::Snapshot(id)) if id == seed);
        assert!(restores, "{}", vm(launch));
    }
    let folders = workload_folders(&case, &journal);
    assert_eq!(
        folders
            .iter()
            .filter(|folder| *folder == "coverage")
            .count(),
        1
    );
    // No probe, and a whole end: logs, `metrics.json`, the chunks.
    assert!(
        execs(&journal)
            .iter()
            .all(|(_, stdout, _)| !stdout.starts_with(&coverage))
    );
    assert!(coverage.join("metrics.json").is_file());
    assert!(coverage.join("api-2.log").is_file());
    assert!(coverage.join("profile/api-2/1.0.coverage.json").is_file());
    assert_clean(&fake, &case.left());

    // Its workload never ends: the repeat ends at 1.5 x `timeout` from its start.
    let case = Case::new(EXAMPLE);
    let fake = crate::fake(example_rules());
    let sandbox = Hooked {
        hang: Some(("load", 3 + 1)),
        ..Hooked::new(&fake)
    };
    let raw = sandbox.run(&case, &case.versions(false)).unwrap();
    assert!(matches!(raw.coverage, Some(CoverageOutcome::Failed(_))));
    let journal = fake.journal();
    let db = &created(&journal)[3 + 4 * 3].1;
    // The first wait ends a chunk of 1 s after the start, the last one at the deadline.
    assert_eq!(waited(&journal, db), (450 - 1) * SECOND);
    assert_clean(&fake, &case.left());
}

#[test]
fn a_coverage_repeat_past_one_and_a_half_timeout_is_a_fact_and_the_run_passes() {
    // The workload of the coverage repeat never ends; `api` leaves a coverage chunk the host could
    // not read, and a failed coverage repeat reads none.
    let case = Case::new(EXAMPLE);
    let fake = fake(chunks(example_rules(), &[("1.0.coverage.json", "[")]));
    let sandbox = Hooked {
        hang: Some(("load", 2 * 3 + 1)),
        ..Hooked::new(&fake)
    };
    let raw = sandbox.run(&case, &case.versions(true)).unwrap();
    let Some(CoverageOutcome::Failed(failure)) = &raw.coverage else {
        panic!("{:?}", raw.coverage)
    };
    let coverage = case.run_dir().join("coverage");
    // No `on-failure` probe either.
    let expected = Failure {
        phase: Phase::Workload,
        service: Some("load".into()),
        repeat: None,
        cause: Cause::Timeout,
        logs: Some(coverage.clone()),
        probes: Vec::new(),
    };
    assert_eq!(failure, &expected);
    assert_eq!(raw.head.len(), 3);
    assert!(matches!(&raw.before, Some(BeforeOutcome::Passed(before)) if before.len() == 3));
    let journal = fake.journal();
    assert!(
        execs(&journal)
            .iter()
            .all(|(_, stdout, _)| !stdout.starts_with(&coverage))
    );
    assert!(coverage.join("load-1.log").is_file());
    assert!(coverage.join("metrics.json").is_file());
    assert!(coverage.join("profile/api-1/1.0.coverage.json").is_file());
    assert_clean(&fake, &case.left());
}

#[test]
fn without_a_profiled_service_there_is_no_coverage_repeat() {
    // No `profile` at all, and one only on the setup service `migrate`, which no repeat runs.
    let plain = EXAMPLE.replace("    x-stand: { profile: node }\n", "");
    let command = "    command: [\"node\", \"dist/migrate.js\"]\n";
    let setup_only = plain.replace(
        command,
        &format!("{command}    x-stand: {{ profile: node }}\n"),
    );
    assert!(plain != EXAMPLE && setup_only != plain);
    for recipe in [plain, setup_only] {
        let case = Case::new(&recipe);
        // Two halves of `timeout` per measured repeat, none for coverage.
        let span = (20 + 3 * 5 * 2 + 10) * 60 * SECOND;
        assert_eq!(run::span(&case.stand), span);
        let fake = fake(example_rules());
        let raw = case.run(&fake, &case.versions(false)).unwrap();
        assert!(raw.coverage.is_none());
        let journal = fake.journal();
        let repeats = ["repeat-1-head", "repeat-2-head", "repeat-3-head"];
        assert_eq!(workload_folders(&case, &journal), repeats);
        assert!(!case.run_dir().join("coverage").exists());
        assert!(!case.run_dir().join("preload").exists());
        for (launch, _) in created(&journal) {
            assert_eq!(profile_mounts(&launch), (None, false), "{}", launch.service);
        }
        assert!(
            !journal
                .iter()
                .any(|call| matches!(call, Call::CopyOut { .. }))
        );
        let mut instances = raw.head.iter().flat_map(|output| &output.instances);
        assert!(instances.all(|instance| instance.profile.is_none()));
        assert_clean(&fake, &case.left());
    }
}

// --- the setup cache ---------------------------------------------------------------------------

/// `side` of `case` on a hit of the entry `built` made.
fn hit(case: &Case, built: &Case, side: Side) -> Version {
    Version {
        tar: case.tar(side),
        setup: SetupCache::Hit(built.entry(side)),
    }
}

/// The snapshot each `db` of the measured repeats of `side` of `case` starts from.
fn repeat_seeds(case: &Case, journal: &[Call], side: Side) -> Vec<SnapshotId> {
    let mut seeds = Vec::new();
    for (launch, _) in created(journal) {
        let folder = out_of(&launch).and_then(Utf8Path::parent);
        let Some(folder) =
            folder.filter(|folder| folder.parent() == Some(case.run_dir().as_path()))
        else {
            continue;
        };
        let name = folder.file_name().unwrap();
        if launch.service == "db"
            && name.starts_with("repeat-")
            && name.ends_with(&side.to_string())
        {
            match &launch.seed {
                Some(Seed::Snapshot(id)) => seeds.push(id.clone()),
                other => panic!("{name}: {other:?}"),
            }
        }
    }
    seeds
}

#[test]
fn a_build_writes_out_and_entry_json_after_the_whole_setup_and_nothing_else() {
    // `db` leaves a folder in `/stand/out` beside the output of `schema`.
    let case = Case::new(EXAMPLE);
    let mut rules = example_rules();
    let db = rules.iter_mut().find(|rule| rule.service == "db").unwrap();
    let rows = ("/stand/out/dump/rows.json".to_owned(), b"[]".to_vec());
    db.script.writes.push(rows);
    let fake = fake(rules);
    case.run(&fake, &case.versions(true)).unwrap();
    let journal = fake.journal();
    for side in [Side::Before, Side::Head] {
        let entry = case.entry(side);
        assert_eq!(names_in(&entry), ["entry.json", "out"], "{side}");
        let copied = files_in(&entry.join("out"));
        let names: Vec<&str> = copied.keys().map(String::as_str).collect();
        let schema = ["probe.schema.jsonl", "probe.schema.jsonl.err"];
        assert_eq!(names, [&["dump/rows.json"][..], &schema].concat());
        let out = case.run_dir().join(format!("setup-{side}/out"));
        assert_eq!(copied, files_in(&out), "{side}");
        // Read back, the ids are the ones the port gave.
        let (db, version) = setup_snapshots(&case, &journal, side);
        let ids = run::cached_ids(&entry).unwrap().unwrap();
        assert!(
            ids.len() == 2 && ids.contains(&db) && ids.contains(&version),
            "{ids:?}"
        );
    }
    // The VMs mount nothing of an entry: setup binds `/stand/out` in the run folder, as without a
    // cache.
    let entries = [case.entry(Side::Head), case.entry(Side::Before)];
    for (launch, _) in created(&journal) {
        for mount in &launch.mounts {
            if let Mount::Bind { host, .. } = mount {
                assert!(
                    !entries.iter().any(|entry| host.starts_with(entry)),
                    "{host}"
                );
            }
        }
    }
    assert_eq!(run::cached_ids(&case.root.join("recipe")).unwrap(), None);
    assert_clean(&fake, &case.left());

    // `entry.json` comes once the whole setup is over: no setup VM sees it, up to its `destroy`;
    // the first VM of the repeats does.
    let case = Case::new(EXAMPLE);
    let fake = crate::fake(example_rules());
    let sandbox = Hooked {
        entry: Some(case.entry(Side::Head).join("entry.json")),
        ..Hooked::new(&fake)
    };
    sandbox.run(&case, &case.versions(false)).unwrap();
    let made = created(&fake.journal());
    let setup: Vec<&SandboxId> = made[..3].iter().map(|(_, id)| id).collect();
    let seen = sandbox.seen.borrow();
    assert!(
        seen[..6]
            .iter()
            .all(|(id, there)| setup.contains(&id) && !there),
        "{seen:?}"
    );
    assert_eq!(seen[6], (made[3].1.clone(), true));
    assert_clean(&fake, &case.left());
}

/// A link a service leaves in `/stand/out` stays out of the entry: a copy would read what it
/// points at on the host.
#[cfg(unix)]
#[test]
fn a_build_copies_no_link_into_the_entry() {
    let case = Case::new(EXAMPLE);
    let out = case.run_dir().join("setup-head/out");
    fs::create_dir_all(&out).unwrap();
    let secret = case.root.join("secret");
    fs::write(&secret, b"host").unwrap();
    std::os::unix::fs::symlink(&secret, out.join("leak")).unwrap();
    let fake = fake(example_rules());
    case.run(&fake, &case.versions(false)).unwrap();
    let copied = names_in(&case.entry(Side::Head).join("out"));
    assert_eq!(copied, ["probe.schema.jsonl", "probe.schema.jsonl.err"]);
    assert_clean(&fake, &case.left());
}

#[test]
fn a_hit_starts_no_setup_vm_and_holds_after_the_run_folder_that_built_it_is_gone() {
    // `db` binds the harness too, and `schema` exits 2: a hit gives that outcome back.
    let recipe = EXAMPLE.replace(
        r#"volumes: ["pgdata:/var/lib/postgresql/data"]"#,
        r#"volumes: ["pgdata:/var/lib/postgresql/data", ".:/harness:ro"]"#,
    );
    let db = Script {
        volume_bytes: 4096,
        probes: vec![
            (
                "probe.schema.jsonl".into(),
                ExecOutcome::Exited(2),
                b"half\n".to_vec(),
            ),
            probe("probe.orders.jsonl", b"{\"id\":1}\n"),
        ],
        ..stops(SECOND)
    };
    let first = Case::new(&recipe);
    let fake = fake(with("db", db));
    first.run(&fake, &first.versions(true)).unwrap();
    let built = fake.journal();
    let seeds = [Side::Before, Side::Head].map(|side| setup_snapshots(&first, &built, side));
    // Of the first run only its entries and its kept snapshots stay: `cli` drops the kept ones
    // once a later run keeps its own.
    let mut left = first.left();
    fs::remove_dir_all(first.run_dir()).unwrap();
    fs::remove_dir_all(first.root.join("recipe")).unwrap();

    let second = Case::new(&recipe);
    let versions = Versions {
        head: hit(&second, &first, Side::Head),
        before: Some(hit(&second, &first, Side::Before)),
    };
    let raw = second.run(&fake, &versions).unwrap();
    let journal = fake.journal();
    let later = &journal[built.len()..];
    // Only repeats: three of each version and the coverage repeat of the head.
    let repeat = ["db-1", "api-1", "api-2", "load-1"];
    assert_eq!(services(later), repeat.repeat(2 * 3 + 1));
    // Each from the entry of its version: `db` from its setup snapshot with the harness of this
    // run, `api` from the version snapshot.
    for vms in created(later).chunks(4) {
        let (db, _) = &vms[0];
        let folder = phase_of(db);
        let (setup, version) = &seeds[usize::from(!folder.ends_with("before"))];
        assert!(
            matches!(&db.seed, Some(Seed::Snapshot(id)) if id == setup),
            "{folder}"
        );
        for (api, _) in &vms[1..3] {
            assert!(matches!(&api.seed, Some(Seed::Snapshot(id)) if id == version));
        }
        let harness = db.mounts.iter().find_map(|mount| match mount {
            Mount::Bind { host, guest, .. } if guest == "/harness" => Some(host),
            _ => None,
        });
        assert!(harness.is_some_and(|host| host.starts_with(&second.root)));
    }
    let setups = [
        (Side::Head, &raw.setup.head),
        (Side::Before, raw.setup.before.as_ref().unwrap()),
    ];
    for (side, setup) in setups {
        let out = second.run_dir().join(format!("setup-{side}/out"));
        assert_eq!(setup.out_dir, out);
        assert_eq!(setup.sizes, [("pgdata".to_owned(), 4096)].into());
        let probes: Vec<(&str, ExecOutcome, &Utf8Path)> = setup
            .probes
            .iter()
            .map(|probe| (probe.name.as_str(), probe.outcome, probe.out.as_path()))
            .collect();
        let schema = out.join("probe.schema.jsonl");
        assert_eq!(
            probes,
            [("schema", ExecOutcome::Exited(2), schema.as_path())]
        );
        assert_eq!(fs::read(&schema).unwrap(), b"half\n");
        assert_eq!(files_in(&out), files_in(&first.entry(side).join("out")));
        assert_eq!(
            names_in(out.parent().unwrap()),
            ["out"],
            "no setup ran in {side}"
        );
    }
    assert!(matches!(&raw.before, Some(BeforeOutcome::Passed(before)) if before.len() == 3));
    left.extend(second.kept());
    assert_clean(&fake, &left);

    // The head built anew beside a hit of "before", as in the live check: one setup runs, the
    // head's, and its entry stays beside the first ones.
    let third = Case::new(&recipe);
    let versions = Versions {
        head: third.version(Side::Head),
        before: Some(hit(&third, &first, Side::Before)),
    };
    let mark = fake.journal().len();
    let raw = third.run(&fake, &versions).unwrap();
    let journal = fake.journal();
    let names = services(&journal[mark..]);
    assert_eq!(&names[..3], ["db-1", "prepare-1", "migrate-1"]);
    let setup = names.iter().filter(|name| !repeat.contains(&name.as_str()));
    assert_eq!(setup.count(), 2, "one prepare and one migrate");
    let before = repeat_seeds(&third, &journal[mark..], Side::Before);
    assert_eq!(
        before,
        vec![seeds[0].0.clone(); 3],
        "from the entry of the first run"
    );
    let sizes = &raw.setup.before.as_ref().unwrap().sizes;
    assert_eq!(*sizes, [("pgdata".to_owned(), 4096)].into());
    left.extend(third.left());
    assert_eq!(
        left.len(),
        3 * 2 + 3 * 2,
        "three entries of two snapshots, three runs that kept `db` of each side"
    );
    assert_clean(&fake, &left);
}

#[test]
fn a_fault_in_writing_or_reading_an_entry_is_a_stand_fault_in_setup() {
    // The entry of the head is a file: its setup passes, the entry cannot be written, no repeat
    // starts, and the snapshots of that setup go.
    let case = Case::new(EXAMPLE);
    let entry = case.entry(Side::Head);
    fs::remove_dir(&entry).unwrap();
    fs::write(&entry, b"").unwrap();
    let fake = fake(example_rules());
    let result = case.run(&fake, &case.versions(false));
    assert!(
        matches!(
            &result,
            Err(RunError::Stand {
                phase: Phase::Setup,
                fault: StandFault::Io { path, .. },
            }) if path.starts_with(&entry)
        ),
        "{result:?}"
    );
    assert_eq!(
        services(&fake.journal()),
        ["db-1", "prepare-1", "migrate-1"]
    );
    assert_clean(&fake, &[]);

    // A hit on an entry without `entry.json`, then on one that does not read: nothing starts.
    let case = Case::new(EXAMPLE);
    let fake = crate::fake(example_rules());
    let versions = Versions {
        head: hit(&case, &case, Side::Head),
        before: None,
    };
    let file = case.entry(Side::Head).join("entry.json");
    for bytes in [None, Some(b"{".as_slice())] {
        if let Some(bytes) = bytes {
            fs::write(&file, bytes).unwrap();
        }
        let result = case.run(&fake, &versions);
        assert!(
            matches!(
                &result,
                Err(RunError::Stand {
                    phase: Phase::Setup,
                    fault: StandFault::Io { path, .. },
                }) if *path == file
            ),
            "{result:?}"
        );
    }
    assert!(fake.journal().is_empty());
    let read = run::cached_ids(&case.entry(Side::Head));
    assert!(
        matches!(&read, Err(StandFault::Io { path, .. }) if *path == file),
        "{read:?}"
    );
}

// --- kept snapshots and inspect ----------------------------------------------------------------

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

#[test]
fn the_last_measured_repeat_of_each_version_keeps_each_volume_service() {
    let case = Case::new(EXAMPLE);
    let fake = fake(example_rules());
    case.run(&fake, &case.versions(true)).unwrap();
    let journal = fake.journal();
    assert_eq!(
        kept_from(&case, &journal),
        [
            ["before", "db", "repeat-3-before"],
            ["head", "db", "repeat-3-head"]
        ]
    );
    // Only `db` has a volume, and no other repeat, the coverage repeat neither, takes a snapshot:
    // setup takes `db` and the version of each side.
    let snapshots = journal
        .iter()
        .filter(|call| matches!(call, Call::Snapshot { .. }));
    assert_eq!(snapshots.count(), 2 * 2 + 2);
    // Each after the stop and the sizes of its VM, before its `destroy`.
    let ids = case.kept();
    assert_eq!(ids.len(), 2);
    for id in &ids {
        let taken = index_of(
            &journal,
            |call| matches!(call, Call::Snapshot { snapshot: Some(own), .. } if own == id),
        );
        let Call::Snapshot { id: vm, .. } = &journal[taken] else {
            unreachable!()
        };
        let at = |matches: &dyn Fn(&Call) -> bool| index_of(&journal, matches);
        let stopped = at(&|call| matches!(call, Call::Stop(made) if made == vm));
        let sized = at(&|call| matches!(call, Call::VolumeSizes(made) if made == vm));
        let gone = at(&|call| matches!(call, Call::Destroy(made) if made == vm));
        assert!(stopped < sized && sized < taken && taken < gone, "{id}");
    }
    // camelCase, with what `inspect` needs of the kept repeat.
    let text = fs::read_to_string(case.run_dir().join("kept.json")).unwrap();
    let kept: serde_json::Value = serde_json::from_str(&text).unwrap();
    let head = &kept[1];
    assert_eq!(head["snapshot"], ids[1].as_str());
    let launch = &head["launch"];
    assert_eq!(
        (&launch["stopSignal"], &launch["stopGraceMs"]),
        (&serde_json::json!("SIGINT"), &serde_json::json!(60_000))
    );
    assert_eq!(launch["health"]["test"], serde_json::json!(["pg_isready"]));
    let out = case.run_dir().join("repeat-3-head").join("out");
    let mounts = serde_json::json!([
        { "kind": "volume", "name": "pgdata", "guest": "/var/lib/postgresql/data" },
        { "kind": "bind", "host": out.as_str(), "guest": "/stand/out", "readOnly": false },
    ]);
    assert_eq!(launch["mounts"], mounts);
    assert_clean(&fake, &case.left());
    assert!(
        run::kept_ids(&case.root.join("nowhere"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_failed_head_repeat_keeps_its_snapshot_and_failed_kept_names_it() {
    // The workload of both versions fails in the first repeat: each keeps `db` of its failure, and
    // `Failed.kept` names only the head's.
    let case = Case::new(EXAMPLE);
    let fake = fake(with("load", exits(1, SECOND)));
    let failed = app_failure(case.run(&fake, &case.versions(true)));
    assert_eq!(failed.kept, ["db"]);
    assert_eq!(
        kept_from(&case, &fake.journal()),
        [
            ["before", "db", "repeat-1-before"],
            ["head", "db", "repeat-1-head"]
        ]
    );
    assert_clean(&fake, &case.left());

    // The head fails in its second repeat, before which "before" passed two: the run ends there,
    // and "before" has no last repeat to keep.
    let case = Case::new(EXAMPLE);
    let fake = crate::fake(example_rules());
    let sandbox = Hooked {
        hang: Some(("load", 4)),
        ..Hooked::new(&fake)
    };
    let failed = app_failure(sandbox.run(&case, &case.versions(true)));
    assert_eq!(failed.failure.repeat, Some(2));
    assert_eq!(failed.kept, ["db"]);
    assert_eq!(
        kept_from(&case, &fake.journal()),
        [["head", "db", "repeat-2-head"]]
    );
    assert!(matches!(&failed.raw.before, Some(BeforeOutcome::Passed(before)) if before.len() == 2));
    assert_clean(&fake, &case.left());
}

#[test]
fn a_kept_json_that_cannot_be_written_breaks_the_stand_and_leaves_no_snapshot() {
    // `kept.json` is a folder: a run that passed is a stand fault at its end, a failed head names
    // no service to inspect; the snapshots no list names go.
    for rules in [example_rules(), with("load", exits(1, SECOND))] {
        let case = Case::new(EXAMPLE);
        let kept = case.run_dir().join("kept.json");
        fs::create_dir(&kept).unwrap();
        let fake = fake(rules);
        match case.run(&fake, &case.versions(false)) {
            Err(RunError::Stand {
                phase: Phase::Workload,
                fault: StandFault::Io { path, .. },
            }) => assert_eq!(path, kept),
            Err(RunError::App(failed)) => assert!(failed.kept.is_empty()),
            other => panic!("{other:?}"),
        }
        assert_clean(&fake, &case.cached());
    }
}

/// `db` with a volume and a probe; `queue` with a volume, the harness, a profile and no
/// healthcheck. The one repeat keeps both.
const KEPT: &str = r#"
services:
  db:
    image: postgres:17
    volumes: ["pgdata:/var/lib/postgresql/data"]
    expose: [5432]
    healthcheck: { test: ["CMD", "pg_isready"], interval: 1s, retries: 2 }
    deploy: { resources: { limits: { cpus: "1", memory: 1g } } }
  queue:
    image: node:22
    volumes: ["jobs:/var/jobs", ".:/harness:ro"]
    command: ["node", "/harness/queue.js"]
    environment: { NODE_OPTIONS: "--max-old-space-size=256" }
    expose: [4000]
    deploy: { resources: { limits: { cpus: "1", memory: 512m } } }
    x-stand: { profile: node }
  load:
    image: node:22
    depends_on:
      db: { condition: service_healthy }
      queue: { condition: service_started }
    deploy: { resources: { limits: { cpus: "1", memory: 512m } } }
volumes: { pgdata: {}, jobs: {} }
x-stand:
  workload: load
  repeat: 1
  probes:
    rows: { in: db, at: after-workload, run: ["psql", "-c", "select 1"] }
"#;

fn kept_rules(db: Script, queue: Script) -> Vec<Rule> {
    vec![
        rule("db", db),
        rule("queue", queue),
        rule("load", exits(0, 5 * SECOND)),
    ]
}

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

#[test]
fn inspect_restores_the_kept_snapshot_without_peers_or_network_and_destroys_it() {
    let case = Case::new(KEPT);
    let db = Script {
        probes: vec![
            probe("1.out", b"3\n"),
            ("2.out".into(), ExecOutcome::Exited(1), Vec::new()),
        ],
        ..stops(SECOND)
    };
    let fake = fake(kept_rules(db, stops(SECOND)));
    case.run(&fake, &case.versions(false)).unwrap();
    let kept = case.kept();
    let mark = fake.journal().len();
    let sandbox = Hooked::new(&fake);
    let inspect = |service: &str, words: &[&str]| {
        let looked = run::inspect(
            &sandbox,
            &case.run_dir(),
            Side::Head,
            service,
            &argv(words),
            &Cancel::new(),
        );
        looked.unwrap()
    };
    let dir = case.run_dir().join("inspect");
    let looked = inspect("db", &["psql", "-c", "select count(*) from orders"]);
    assert_eq!(looked.outcome, ExecOutcome::Exited(0));
    assert_eq!(looked.out, dir.join("1.out"));
    assert_eq!(fs::read(&looked.out).unwrap(), b"3\n");
    // The next look writes the next file; a code other than 0 is what its command gave.
    let again = inspect("db", &["false"]);
    assert_eq!(
        (again.outcome, again.out),
        (ExecOutcome::Exited(1), dir.join("2.out"))
    );
    let queue = inspect("queue", &["ls", "/var/jobs"]);
    assert_eq!(queue.out, dir.join("3.out"));
    assert_eq!(*sandbox.deadlines.borrow(), [60 * SECOND; 3]);
    // Each look is one VM from its kept snapshot with the launch of the kept repeat, but no peers,
    // no ports, no network and `/stand/out` on `inspect/`; it is ready first, runs `argv`, goes.
    let journal = fake.journal();
    let made = created(&journal[..mark]);
    let looks = created(&journal[mark..]);
    let expected = [("db", &kept[0]), ("db", &kept[0]), ("queue", &kept[1])];
    assert_eq!(looks.len(), expected.len());
    for ((launch, id), (service, snapshot)) in looks.iter().zip(expected) {
        let taken = journal.iter().find_map(|call| match call {
            Call::Snapshot {
                id,
                snapshot: Some(own),
            } if own == snapshot => Some(id),
            _ => None,
        });
        let (repeat, _) = made.iter().find(|(_, made)| Some(made) == taken).unwrap();
        assert_eq!(repeat.service, service);
        let mut restored = repeat.clone();
        restored.peers = Vec::new();
        restored.ports = Vec::new();
        restored.outbound = sandbox::Outbound::Off;
        restored.seed = Some(Seed::Snapshot(snapshot.clone()));
        restored
            .mounts
            .retain(|mount| !matches!(mount, Mount::Bind { guest, .. } if guest == "/stand/out"));
        restored.mounts.push(Mount::Bind {
            host: dir.clone(),
            guest: "/stand/out".into(),
            read_only: false,
        });
        assert_eq!(format!("{launch:?}"), format!("{restored:?}"), "{service}");
        let on = |matches: &dyn Fn(&Call) -> bool| index_of(&journal, matches);
        let ran = on(&|call| matches!(call, Call::Exec { id: made, .. } if made == id));
        let gone = on(&|call| matches!(call, Call::Destroy(made) if made == id));
        assert!(ran < gone, "{service}");
        let checked = journal
            .iter()
            .position(|call| matches!(call, Call::ExecDetached { id: made, .. } if made == id));
        assert_eq!(
            checked.is_some_and(|checked| checked < ran),
            service == "db"
        );
    }
    // `queue` binds the harness and the preload of the run again, and has its chunks folder.
    let (queue, _) = &looks[2];
    let binds: Vec<(&str, &Utf8Path, bool)> = queue
        .mounts
        .iter()
        .filter_map(|mount| match mount {
            Mount::Bind {
                host,
                guest,
                read_only,
            } => Some((guest.as_str(), host.as_path(), *read_only)),
            _ => None,
        })
        .collect();
    let preload = case.run_dir().join("preload");
    let recipe = case.root.join("recipe");
    assert_eq!(
        binds,
        [
            ("/harness", recipe.as_path(), true),
            ("/stand/preload", preload.as_path(), true),
            ("/stand/out", dir.as_path(), false)
        ]
    );
    assert_eq!(profile_mounts(queue), (Some(preload.as_path()), true));
    // Nothing reaches the snapshots.
    let later = &journal[mark..];
    assert!(
        !later
            .iter()
            .any(|call| matches!(call, Call::Snapshot { .. } | Call::DropSnapshot(_)))
    );
    assert_left(&fake, &case.left());
}

#[test]
fn inspect_refuses_what_is_not_kept_and_a_gone_harness_folder_creates_nothing() {
    let case = Case::new(KEPT);
    let fake = fake(kept_rules(stops(SECOND), stops(SECOND)));
    let cancel = Cancel::new();
    let inspect = |run_dir: &Utf8Path, side: Side, service: &str| {
        run::inspect(&fake, run_dir, side, service, &argv(&["true"]), &cancel)
    };
    // A run folder without `kept.json`, none at all, a side, a service without a volume, no such
    // service.
    let refused = inspect(&case.run_dir(), Side::Head, "db");
    assert!(
        matches!(refused, Err(InspectError::NotKept(_))),
        "{refused:?}"
    );
    case.run(&fake, &case.versions(false)).unwrap();
    let left = case.left();
    let mark = fake.journal().len();
    let (nowhere, run_dir) = (case.root.join("nowhere"), case.run_dir());
    let asked = [
        (nowhere.as_path(), Side::Head, "db"),
        (run_dir.as_path(), Side::Before, "db"),
        (run_dir.as_path(), Side::Head, "load"),
        (run_dir.as_path(), Side::Head, "api"),
    ];
    for (run_dir, side, service) in asked {
        let refused = inspect(run_dir, side, service);
        assert!(
            matches!(&refused, Err(InspectError::NotKept(_))),
            "{service}: {refused:?}"
        );
    }
    // The harness folder is gone, then the preload of the run: `NotStarted` with its path, and no
    // VM; `db`, which binds neither, still starts.
    let recipe = case.root.join("recipe");
    let preload = case.run_dir().join("preload");
    fs::remove_dir_all(&recipe).unwrap();
    for gone in [&recipe, &preload] {
        if *gone == preload {
            fs::create_dir(&recipe).unwrap();
            fs::remove_dir_all(&preload).unwrap();
        }
        let made = fake.journal().len();
        match inspect(&case.run_dir(), Side::Head, "queue") {
            Err(InspectError::NotStarted { service, cause }) => {
                assert_eq!(service, "queue");
                assert_eq!(cause, format!("its folder {gone} is gone"));
            }
            other => panic!("{other:?}"),
        }
        assert!(created(&fake.journal()[made..]).is_empty(), "{gone}");
    }
    inspect(&case.run_dir(), Side::Head, "db").unwrap();
    assert_eq!(created(&fake.journal()[mark..]).len(), 1);
    assert_left(&fake, &left);
    // A `kept.json` that does not read breaks the stand.
    let file = case.run_dir().join("kept.json");
    fs::write(&file, "[").unwrap();
    let refused = inspect(&case.run_dir(), Side::Head, "db");
    assert!(
        matches!(&refused, Err(InspectError::Stand(StandFault::Io { path, .. })) if *path == file),
        "{refused:?}"
    );
    let read = run::kept_ids(&case.run_dir());
    assert!(
        matches!(&read, Err(StandFault::Io { path, .. }) if *path == file),
        "{read:?}"
    );
}

#[test]
fn inspect_of_a_service_that_exits_or_stays_unhealthy_did_not_start_and_cancel_destroys_it() {
    // Each head fails in its repeat, which keeps both; the restored VM meets the same script:
    // `queue` exits at once, `db` fails its healthcheck, `db`'s attempts hang past 60 s.
    let exits_at_once = Script {
        exit: Some((1, Duration::ZERO)),
        ..stops(SECOND)
    };
    let unhealthy = Script {
        health: vec![Some(1)],
        ..stops(SECOND)
    };
    let hanging = Script {
        health: vec![None],
        ..stops(SECOND)
    };
    let cases = [
        (stops(SECOND), exits_at_once, "queue", "it exited with 1"),
        (unhealthy, stops(SECOND), "db", "it stayed unhealthy"),
        (
            hanging,
            stops(SECOND),
            "db",
            "it was not healthy within 60 s",
        ),
    ];
    for (db, queue, service, cause) in cases {
        let case = Case::new(KEPT);
        let fake = fake(kept_rules(db, queue));
        let failed = app_failure(case.run(&fake, &case.versions(false)));
        assert_eq!(failed.kept, ["db", "queue"]);
        let mark = fake.journal().len();
        let start = fake.now();
        let refused = run::inspect(
            &fake,
            &case.run_dir(),
            Side::Head,
            service,
            &argv(&["true"]),
            &Cancel::new(),
        );
        let Err(InspectError::NotStarted {
            service: own,
            cause: said,
        }) = refused
        else {
            panic!("{service}: {refused:?}")
        };
        assert_eq!(own, service);
        assert_eq!(said, format!("{cause}, without its peers and network"));
        if cause.contains("60 s") {
            assert_eq!(fake.now() - start, 60 * SECOND);
        }
        let later = &fake.journal()[mark..];
        assert_eq!(created(later).len(), 1, "{service}");
        assert!(!later.iter().any(|call| matches!(call, Call::Exec { .. })));
        assert_left(&fake, &case.left());
    }

    // Ctrl+C once the VM is created, with a healthcheck and without: it goes, and nothing runs
    // in it; set before, no VM starts.
    let case = Case::new(KEPT);
    let fake = fake(kept_rules(stops(SECOND), stops(SECOND)));
    case.run(&fake, &case.versions(false)).unwrap();
    for (service, at_create) in [("db", true), ("queue", true), ("db", false)] {
        let sandbox = Hooked {
            cancel_at: at_create.then_some(service),
            ..Hooked::new(&fake)
        };
        if !at_create {
            sandbox.cancel.cancel();
        }
        let mark = fake.journal().len();
        let refused = run::inspect(
            &sandbox,
            &case.run_dir(),
            Side::Head,
            service,
            &argv(&["true"]),
            &sandbox.cancel,
        );
        assert!(
            matches!(refused, Err(InspectError::Cancelled)),
            "{service}: {refused:?}"
        );
        let later = &fake.journal()[mark..];
        assert_eq!(created(later).len(), usize::from(at_create), "{service}");
        assert!(!later.iter().any(|call| matches!(call, Call::Exec { .. })));
        assert_left(&fake, &case.left());
    }
}

// --- the repeats outside the stats -------------------------------------------------------------

/// `EXAMPLE` with the fault on `db:5432` and the second pass: the stand asks for every repeat
/// outside the stats.
fn every_repeat() -> String {
    EXAMPLE.replace(
        "  workload: load\n",
        "  workload: load\n  fault: { target: \"db:5432\", mode: down }\n  second_pass: true\n",
    )
}

/// The folder of each phase past the measured repeats that started the workload, in order.
fn outside_folders(case: &Case, journal: &[Call]) -> Vec<String> {
    let folders = workload_folders(case, journal).into_iter();
    folders
        .filter(|folder| !folder.starts_with("repeat-"))
        .collect()
}

#[test]
fn the_set_of_extras_runs_only_what_it_names_and_the_stand_asks_for() {
    let span = run::span(&Case::new(&every_repeat()).stand);
    let sets: [(&[Extra], &[&str]); 4] = [
        (&[], &[]),
        (&[Extra::Coverage], &["coverage"]),
        (&[Extra::Fault, Extra::SecondPass], &[]),
        (
            &[Extra::Coverage, Extra::Fault, Extra::SecondPass],
            &["coverage"],
        ),
    ];
    for (set, folders) in sets {
        let case = Case::new(&every_repeat());
        let fake = fake(example_rules());
        let extras: BTreeSet<Extra> = set.iter().copied().collect();
        let raw = case.run_with(&fake, &case.versions(true), &extras).unwrap();
        let journal = fake.journal();
        assert_eq!(outside_folders(&case, &journal), folders, "{set:?}");
        assert_eq!(raw.coverage.is_some(), set.contains(&Extra::Coverage));
        assert!(matches!(&raw.before, Some(BeforeOutcome::Passed(before)) if before.len() == 3));
        assert_eq!(raw.head.len(), 3);
        // `span` is of the stand: the same for every set, the repeats it does not run included.
        for (launch, _) in created(&journal) {
            assert_eq!(launch.max_duration, span, "{set:?}");
        }
        assert_clean(&fake, &case.left());
    }

    // A stand that asks for none of them runs only the measured repeats, whatever the set says.
    let case = Case::new(&EXAMPLE.replace("    x-stand: { profile: node }\n", ""));
    let fake = fake(example_rules());
    let raw = case.run(&fake, &case.versions(true)).unwrap();
    assert!(raw.coverage.is_none());
    assert!(outside_folders(&case, &fake.journal()).is_empty());
    assert_clean(&fake, &case.left());
}
