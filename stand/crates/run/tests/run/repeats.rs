//! The measured repeats: their turns and levels, `wall`, their end, and a failure, `Cancel` or a
//! stand fault in one.

use std::collections::BTreeSet;
use std::time::Duration;

use camino::Utf8PathBuf;
use report::{BeforeOutcome, Cause, Phase, Role};
use run::{RunError, Side, StandFault};
use sandbox::{Call, Fake, Launch, Mount, Rule, SandboxError, SandboxId, Scenario, Script, Seed};

use super::{
    AFTER_THE_WORKLOAD, Case, EXAMPLE, Hooked, SECOND, app_failure, assert_clean, assert_left,
    created, end, example_rules, exits, fake, index_of, kept_from, logs_in, metrics_in, names,
    on_before, phase_of, rule, services, snapshot_of, stopped, stops, vm, waited, with,
    workload_folders,
};

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

    // `db` ends the moment the workload exits 0, 30 s after both started: no wait gave its exit,
    // the look before the cut does, and the repeat it would have measured fails.
    let case = Case::new(EXAMPLE);
    let fake = crate::fake(example_rules());
    let sandbox = Hooked {
        ends: vec![end("db", 2, 1, 30 * SECOND)],
        ..Hooked::new(&fake)
    };
    let failed = app_failure(sandbox.run(&case, &case.versions(false)));
    let failure = &failed.failure;
    assert_eq!(
        (
            failure.phase,
            failure.service.as_deref(),
            &failure.cause,
            failure.repeat
        ),
        (Phase::Workload, Some("db"), &Cause::Died, Some(1))
    );
    assert_eq!(failed.raw.head[0].wall, 30 * SECOND);
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
