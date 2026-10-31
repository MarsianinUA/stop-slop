//! Setup of each version, what every launch carries (`span`, the stop of its service), and how a
//! failure of setup or of "before" ends.

use std::collections::BTreeSet;
use std::fs;
use std::time::Duration;

use report::{BeforeOutcome, Cause, Phase};
use run::{RunError, Side, StandFault};
use sandbox::{Call, Fake, Launch, Sandbox, SandboxError, Scenario, Script, Seed, Waitable};

use super::{
    Case, EXAMPLE, Hooked, NO_PREPARE, SECOND, app_failure, assert_clean, assert_left, created,
    end, every_repeat, example_rules, exits, fake, index_of, metrics_in, names_in,
    no_prepare_rules, on_before, replaced, rule, snapshot_of, stopped, stops, with,
    workload_folders,
};

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
    // `spec` bounds `repeat` by `u32` only: the halves saturate, they never wrap.
    let most = Case::new(&every_repeat().replace("  repeat: 3\n", "  repeat: 4294967295\n"));
    assert_eq!(
        run::span(&most.stand),
        Duration::from_secs(20 * 60 + u64::from(u32::MAX) * 150 + 10 * 60)
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

    // `db` ends the moment `migrate` exits 0, 22 s after `db` started: no wait gave its exit, the
    // look after the last setup service does, and no snapshot keeps the crashed database.
    let case = Case::new(EXAMPLE);
    let fake = crate::fake(example_rules());
    let sandbox = Hooked {
        ends: vec![end("db", 1, 1, 22 * SECOND)],
        ..Hooked::new(&fake)
    };
    let failed = app_failure(sandbox.run(&case, &case.versions(false)));
    let failure = &failed.failure;
    assert_eq!(
        (failure.phase, failure.service.as_deref(), &failure.cause),
        (Phase::Setup, Some("db"), &Cause::Died)
    );
    assert_eq!(run::cached_ids(&case.entry(Side::Head)).unwrap(), None);
    let journal = fake.journal();
    let db = &created(&journal)[0];
    assert_eq!(db.0.service, "db");
    let snapshots = journal.iter().filter(|call| match call {
        Call::Snapshot { id, .. } | Call::VolumeSizes(id) => *id == db.1,
        _ => false,
    });
    assert_eq!(snapshots.count(), 0);
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
