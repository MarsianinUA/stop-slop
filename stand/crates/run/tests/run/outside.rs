//! The repeats outside the stats: the set of extras, the fault repeats, the second pass.

use std::collections::BTreeSet;
use std::time::Duration;

use camino::Utf8Path;
use report::{BeforeOutcome, Cause, Extra, Failure, PassPair, Phase};
use run::{RunError, StandFault};
use sandbox::{
    Call, Fault, FaultMode, Launch, Metrics, Mount, PortStats, Rule, SandboxError, SandboxId,
    Script,
};

use super::{
    AFTER_THE_WORKLOAD, Case, EXAMPLE, End, Hooked, SECOND, assert_clean, assert_left, created,
    end, every_repeat, example_rules, execs, exits, fake, index_of, kept_from, killed, logs_in,
    names, names_in, out_of, phase_of, profile_mounts, ran, replaced, rule, stops, vm, waited,
    with_x_stand, workload_folders,
};

/// The folders of the fault repeats, in their turns.
const FAULTS: [&str; 4] = [
    "fault-1-before",
    "fault-1-head",
    "fault-2-before",
    "fault-2-head",
];

/// The folder of each start of the workload in the second passes: two per pass.
const PASSES: [&str; 4] = ["pass2-before", "pass2-before", "pass2-head", "pass2-head"];

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
    let both = [FAULTS.as_slice(), &PASSES].concat();
    let every = [&["coverage"][..], &both].concat();
    let sets: [(&[Extra], &[&str]); 6] = [
        (&[], &[]),
        (&[Extra::Coverage], &["coverage"]),
        (&[Extra::Fault], &FAULTS),
        (&[Extra::SecondPass], &PASSES),
        (&[Extra::Fault, Extra::SecondPass], &both),
        (&[Extra::Coverage, Extra::Fault, Extra::SecondPass], &every),
    ];
    for (set, folders) in sets {
        let case = Case::new(&every_repeat());
        let fake = fake(example_rules());
        let extras: BTreeSet<Extra> = set.iter().copied().collect();
        let raw = case.run_with(&fake, &case.versions(true), &extras).unwrap();
        let journal = fake.journal();
        assert_eq!(outside_folders(&case, &journal), folders, "{set:?}");
        assert_eq!(raw.coverage.is_some(), set.contains(&Extra::Coverage));
        assert_eq!(raw.fault.is_some(), set.contains(&Extra::Fault));
        assert_eq!(raw.second_pass.is_some(), set.contains(&Extra::SecondPass));
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
    assert!(raw.coverage.is_none() && raw.fault.is_none() && raw.second_pass.is_none());
    assert!(outside_folders(&case, &fake.journal()).is_empty());
    assert_clean(&fake, &case.left());
}

/// `EXAMPLE` with `fault` on `db:5432`.
fn under_fault(fault: &str) -> String {
    with_x_stand(&format!("  fault: {{ target: \"db:5432\", {fault} }}\n"))
}

/// Metrics whose only port is `target`, with `connections` dropped before their first byte.
fn refused(target: &str, connections: u64) -> Metrics {
    Metrics {
        ports: vec![PortStats {
            target: target.into(),
            connections,
            open: 0,
            bytes_out: 0,
            bytes_in: 0,
            http: None,
            drops: connections,
        }],
        ..Metrics::default()
    }
}

/// `example_rules` where `api` and the workload say what their relays see under a fault.
fn fault_rules() -> Vec<Rule> {
    let api = Script {
        under_fault: Some(refused("db:5432", 7)),
        ..stops(100 * Duration::from_millis(1))
    };
    let load = Script {
        under_fault: Some(refused("api:3000", 9)),
        writes: vec![("/stand/out/orders.jsonl".into(), b"{}\n".to_vec())],
        ..exits(0, 30 * SECOND)
    };
    replaced(replaced(example_rules(), "api", api), "load", load)
}

/// Each `Fault` of the journal: its index, the VMs it names as `service-replica`, its switch.
fn faults(journal: &[Call]) -> Vec<(usize, Vec<String>, Fault)> {
    let made = created(journal);
    let name = |id: &SandboxId| {
        let (launch, _) = made.iter().find(|(_, made)| made == id).unwrap();
        vm(launch)
    };
    let faults = journal.iter().enumerate();
    let faults = faults.filter_map(|(index, call)| match call {
        Call::Fault { ids, fault } => Some((index, ids.iter().map(name).collect(), fault.clone())),
        _ => None,
    });
    faults.collect()
}

#[test]
fn two_fault_repeats_per_version_take_turns_after_the_measured_ones_with_the_fault_on_before_the_workload()
 {
    let case = Case::new(&under_fault("mode: down"));
    let fake = fake(fault_rules());
    let raw = case.run(&fake, &case.versions(true)).unwrap();
    let journal = fake.journal();
    let mut folders = vec!["coverage"];
    folders.extend(FAULTS);
    assert_eq!(outside_folders(&case, &journal), folders);
    let Some(fault) = &raw.fault else {
        panic!("no fault outcome")
    };
    assert_eq!(
        (fault.target.as_str(), fault.mode.as_str()),
        ("db:5432", "down")
    );
    let before = fault.before.as_ref().expect("the fault repeats of before");
    for repeats in [&fault.head, before] {
        let numbers: Vec<u32> = repeats.iter().map(|repeat| repeat.repeat).collect();
        assert_eq!(numbers, [1, 2]);
        for repeat in repeats {
            assert_eq!(
                (repeat.workload, repeat.exited.len(), &repeat.failure),
                (Some(0), 0, &None)
            );
            let seen: Vec<(&str, u32, u64)> = repeat
                .instances
                .iter()
                .map(|instance| {
                    let dropped = instance.metrics.ports.iter().map(|port| port.drops);
                    (instance.service.as_str(), instance.replica, dropped.sum())
                })
                .collect();
            // The relays of the app see the fault; the workload, which drives the repeat, does not.
            let drops = [("db", 1, 0), ("api", 1, 7), ("api", 2, 7), ("load", 1, 0)];
            assert_eq!(seen, drops);
            let profiles = repeat.instances.iter().map(|instance| &instance.profile);
            assert!(profiles.into_iter().all(Option::is_none));
        }
    }
    // The last four repeats; in each, one `fault` on every VM but the workload, once `api` is
    // healthy and before the workload is created.
    let made = created(&journal);
    let switches = faults(&journal);
    assert_eq!(switches.len(), 4);
    let last = made[made.len() - 16..].chunks(4);
    for (chunk, ((index, vms, switch), folder)) in last.zip(switches.iter().zip(FAULTS)) {
        let names: Vec<String> = chunk.iter().map(|(launch, _)| vm(launch)).collect();
        assert_eq!(names, ["db-1", "api-1", "api-2", "load-1"]);
        assert_eq!(
            (phase_of(&chunk[0].0), phase_of(&chunk[3].0)),
            (folder, folder)
        );
        assert_eq!(vms, &["db-1", "api-1", "api-2"]);
        assert_eq!(
            (switch.target.as_str(), switch.mode),
            ("db:5432", FaultMode::Down)
        );
        let healthy = journal[..*index]
            .iter()
            .any(|call| matches!(call, Call::ExecDetached { id, .. } if *id == chunk[2].1));
        let load = index_of(
            &journal,
            |call| matches!(call, Call::Create { ids, .. } if ids.contains(&chunk[3].1)),
        );
        assert!(healthy && *index < load, "{folder}");
        // No profiler, no probe, no kept snapshot; the end is whole.
        for (launch, _) in chunk {
            assert_eq!(profile_mounts(launch), (None, false), "{folder}");
        }
        let dir = case.run_dir().join(folder);
        let probed = execs(&journal);
        assert!(
            probed
                .iter()
                .all(|(_, stdout, _)| !stdout.starts_with(&dir))
        );
        assert!(dir.join("metrics.json").is_file() && dir.join("api-2.log").is_file());
        assert!(dir.join("out/orders.jsonl").is_file());
    }
    assert_eq!(
        kept_from(&case, &journal),
        [
            ["before", "db", "repeat-3-before"],
            ["head", "db", "repeat-3-head"]
        ]
    );
    assert_clean(&fake, &case.left());

    // `slow` with its delay, and only the head without `--before`.
    let case = Case::new(&under_fault("mode: slow, delay: 2s"));
    let fake = crate::fake(fault_rules());
    let raw = case.run(&fake, &case.versions(false)).unwrap();
    let fault = raw.fault.unwrap();
    assert_eq!((fault.mode.as_str(), fault.head.len()), ("slow 2000ms", 2));
    assert!(fault.before.is_none());
    let switches = faults(&fake.journal());
    assert_eq!(switches.len(), 2);
    for (_, _, switch) in switches {
        assert_eq!(switch.mode, FaultMode::Slow(2 * SECOND));
    }
    assert_clean(&fake, &case.left());

    // A delay finer than a millisecond keeps its fraction in the mode.
    let case = Case::new(&under_fault("mode: slow, delay: 1500us"));
    let fake = crate::fake(fault_rules());
    let raw = case.run(&fake, &case.versions(false)).unwrap();
    assert_eq!(raw.fault.unwrap().mode, "slow 1.5ms");
    assert_clean(&fake, &case.left());

    // `cache` starts with the workload and `side` after it: each gets the fault right after its
    // `create`, the workload never.
    let case = Case::new(&AFTER_THE_WORKLOAD.replace(
        "x-stand: { workload: load }",
        "x-stand: { workload: load, repeat: 1, fault: { target: \"db:5432\", mode: down } }",
    ));
    let fake = crate::fake(vec![
        rule("db", stops(SECOND)),
        rule("cache", stops(SECOND)),
        rule("side", stops(SECOND)),
        rule("load", exits(0, 5 * SECOND)),
    ]);
    case.run(&fake, &case.versions(false)).unwrap();
    let journal = fake.journal();
    let switches = faults(&journal);
    let mut seen = Vec::new();
    for (index, vms, _) in &switches {
        let last = journal[..*index]
            .iter()
            .rposition(|call| matches!(call, Call::Create { .. }));
        let Call::Create { batch, .. } = &journal[last.unwrap()] else {
            unreachable!()
        };
        let batch: Vec<&str> = batch.iter().map(|launch| launch.service.as_str()).collect();
        seen.push((vms.join(" "), batch.join(" ")));
    }
    let seen: Vec<(&str, &str)> = seen
        .iter()
        .map(|(vms, batch)| (vms.as_str(), batch.as_str()))
        .collect();
    let each = [
        ("db-1", "db"),
        ("cache-1", "cache load"),
        ("side-1", "side"),
    ];
    assert_eq!(seen, [each, each].concat());
    assert_clean(&fake, &case.left());
}

/// `api` fails its first healthcheck, so it is healthy a second after its start at the earliest.
fn healthy_late() -> Script {
    Script {
        health: vec![Some(1), Some(0)],
        ..stops(SECOND)
    }
}

/// The index of the first VM of the phase in `folder`, among those that bind `/stand/out`.
fn first_in(made: &[(Launch, SandboxId)], folder: &str) -> usize {
    let mut made = made.iter();
    let first = made.position(|(launch, _)| out_of(launch).is_some() && phase_of(launch) == folder);
    first.unwrap()
}

#[test]
fn a_fault_repeat_turns_the_workload_code_its_timeout_and_a_dead_service_into_categories() {
    // Head only. By service, the VMs of setup come first, then those of the three measured repeats
    // and the coverage repeat: the first fault repeat makes the fifth `load` and the ninth and
    // tenth `api`.
    /// `hang` and `ends` of `Hooked`, and the categories of the first fault repeat: its failure as
    /// phase, service and cause.
    struct Ending {
        rules: Vec<Rule>,
        hang: Option<(&'static str, usize)>,
        ends: Vec<End>,
        workload: Option<i32>,
        exited: &'static [&'static str],
        failure: Option<(Phase, &'static str, Cause)>,
    }
    let cases = [
        // The workload ends with 3.
        Ending {
            rules: fault_rules(),
            hang: None,
            ends: vec![end("load", 5, 3, 10 * SECOND)],
            workload: Some(3),
            exited: &[],
            failure: Some((Phase::Workload, "load", Cause::Exit(3))),
        },
        // The workload hangs until the deadline of the repeat.
        Ending {
            rules: fault_rules(),
            hang: Some(("load", 5)),
            ends: Vec::new(),
            workload: None,
            exited: &[],
            failure: Some((Phase::Workload, "load", Cause::Timeout)),
        },
        // The workload is killed without a code.
        Ending {
            rules: fault_rules(),
            hang: None,
            ends: vec![killed("load", 5, 10 * SECOND)],
            workload: None,
            exited: &[],
            failure: Some((Phase::Workload, "load", Cause::Died)),
        },
        // `api` dies under the fault, and the repeat goes on until the workload ends.
        Ending {
            rules: fault_rules(),
            hang: None,
            ends: vec![end("api", 9, 1, 5 * SECOND)],
            workload: Some(0),
            exited: &["api"],
            failure: None,
        },
        // Both replicas of `api` die under the fault: one service ended.
        Ending {
            rules: fault_rules(),
            hang: None,
            ends: vec![end("api", 9, 1, 5 * SECOND), end("api", 10, 1, 6 * SECOND)],
            workload: Some(0),
            exited: &["api"],
            failure: None,
        },
        // `db` and `api` die the moment the workload ends, 30 s after all three started: no wait
        // gave their exits yet, the look before the cut does.
        Ending {
            rules: fault_rules(),
            hang: None,
            ends: vec![end("db", 6, 1, 30 * SECOND), end("api", 9, 1, 30 * SECOND)],
            workload: Some(0),
            exited: &["db", "api"],
            failure: None,
        },
        // The same as the workload ends with 3: the look before the cut notes them too.
        Ending {
            rules: fault_rules(),
            hang: None,
            ends: vec![
                end("db", 6, 1, 30 * SECOND),
                end("api", 9, 1, 30 * SECOND),
                end("load", 5, 3, 30 * SECOND),
            ],
            workload: Some(3),
            exited: &["db", "api"],
            failure: Some((Phase::Workload, "load", Cause::Exit(3))),
        },
        // `api` dies before it is healthy, so before the fault: the workload never starts, and
        // the phase says the fault had no part in it.
        Ending {
            rules: replaced(fault_rules(), "api", healthy_late()),
            hang: None,
            ends: vec![end("api", 9, 1, SECOND / 2)],
            workload: None,
            exited: &["api"],
            failure: Some((Phase::Ready, "api", Cause::Died)),
        },
    ];
    for Ending {
        rules,
        hang,
        ends,
        workload,
        exited,
        failure,
    } in cases
    {
        let case = Case::new(&under_fault("mode: down"));
        let fake = fake(rules);
        let sandbox = Hooked {
            hang,
            ends: ends.clone(),
            ..Hooked::new(&fake)
        };
        let raw = sandbox.run(&case, &case.versions(false)).unwrap();
        let fault = raw.fault.unwrap();
        let [first, second] = &fault.head[..] else {
            panic!("{:?}", fault.head)
        };
        let dir = case.run_dir().join("fault-1-head");
        assert_eq!(first.workload, workload, "{hang:?} {ends:?}");
        assert_eq!(first.exited, exited, "{hang:?} {ends:?}");
        let failure = failure.map(|(phase, service, cause)| Failure {
            phase,
            service: Some(service.into()),
            repeat: Some(1),
            cause,
            logs: Some(dir.clone()),
            probes: Vec::new(),
        });
        assert_eq!(first.failure, failure, "{hang:?} {ends:?}");
        assert_eq!(
            (second.workload, second.exited.len(), &second.failure),
            (Some(0), 0, &None)
        );
        assert_eq!(raw.head.len(), 3);
        assert!(dir.join("metrics.json").is_file() && dir.join("db-1.log").is_file());
        let journal = fake.journal();
        if hang.is_some() {
            // The deadline is `timeout` from the start of the repeat.
            let made = created(&journal);
            let db = &made[first_in(&made, "fault-1-head")].1;
            assert_eq!(waited(&journal, db), 299 * SECOND);
        }
        assert_clean(&fake, &case.left());
    }

    // A switch the port cannot set breaks the stand: before the workload of the repeat started,
    // and on `cache`, created with the workload, after it.
    let after = AFTER_THE_WORKLOAD.replace(
        "x-stand: { workload: load }",
        "x-stand: { workload: load, repeat: 1, fault: { target: \"db:5432\", mode: down } }",
    );
    let after_rules = vec![
        rule("db", stops(SECOND)),
        rule("cache", stops(SECOND)),
        rule("side", stops(SECOND)),
        rule("load", exits(0, 5 * SECOND)),
    ];
    let cases = [
        (
            under_fault("mode: down"),
            fault_rules(),
            1,
            Phase::Ready,
            &["db-1", "api-1", "api-2"][..],
        ),
        (
            after,
            after_rules,
            2,
            Phase::Workload,
            &["db-1", "cache-1", "load-1"][..],
        ),
    ];
    for (recipe, rules, nth, phase, logs) in cases {
        let case = Case::new(&recipe);
        let fake = fake(rules);
        let sandbox = Hooked {
            fail_fault: Some(nth),
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
            "{result:?}"
        );
        let dir = case.run_dir().join("fault-1-head");
        assert_eq!(logs_in(&dir), names(logs));
        assert_left(&fake, &case.left());
    }
}

/// `EXAMPLE` with the second pass and two files that should not change; the workload publishes a
/// port, so the `Fake` refuses a second live VM of it.
fn twice() -> String {
    let unchanged = "{ unchanged: [{ file: probe.orders.jsonl, reason: \"a read\" }, \
                     { file: orders.jsonl, reason: \"the same orders\" }] }";
    with_x_stand(&format!("  second_pass: true\n  expect: {unchanged}\n")).replace(
        "    command: [\"node\", \"load.js\", \"http://api:3000\"]\n",
        "    command: [\"node\", \"load.js\", \"http://api:3000\"]\n    expose: [9000]\n",
    )
}

/// Metrics whose only port is `target`, with `open` connections still open.
fn holding(target: &str, open: u64) -> Metrics {
    Metrics {
        ports: vec![PortStats {
            target: target.into(),
            connections: 10,
            open,
            bytes_out: 100,
            bytes_in: 200,
            http: None,
            drops: 0,
        }],
        ..Metrics::default()
    }
}

/// `example_rules` where `api` holds connections to `db` and the workload still holds its own to
/// `api` at the cut, as the relay closes them up to 0.5 s after its exit.
fn pass_rules() -> Vec<Rule> {
    let api = Script {
        metrics: holding("db:5432", 2),
        ..stops(100 * Duration::from_millis(1))
    };
    let load = Script {
        metrics: holding("api:3000", 3),
        writes: vec![(
            "/stand/out/orders.jsonl".into(),
            b"{\"ok\":true}\n".to_vec(),
        )],
        ..exits(0, 30 * SECOND)
    };
    replaced(replaced(example_rules(), "api", api), "load", load)
}

#[test]
fn the_second_pass_runs_the_workload_twice_on_one_state() {
    let case = Case::new(&twice());
    let fake = fake(pass_rules());
    let raw = case.run(&fake, &case.versions(true)).unwrap();
    let journal = fake.journal();
    assert_eq!(
        outside_folders(&case, &journal),
        [&["coverage"][..], &PASSES].concat()
    );
    let Some(pass) = &raw.second_pass else {
        panic!("no second pass")
    };
    assert_eq!(pass.unchanged, ["probe.orders.jsonl", "orders.jsonl"]);
    let Some(before) = &pass.before else {
        panic!("no second pass of before")
    };
    // The last ten VMs: per version `db`, both `api`, then the workload twice.
    let made = created(&journal);
    let last = made[made.len() - 10..].chunks(5);
    for ((pair, chunk), side) in [before, &pass.head]
        .into_iter()
        .zip(last)
        .zip(["before", "head"])
    {
        let PassPair::Passed { first, second } = pair else {
            panic!("{side}: {pair:?}")
        };
        let dir = case.run_dir().join(format!("pass2-{side}"));
        let vms: Vec<String> = chunk.iter().map(|(launch, _)| vm(launch)).collect();
        assert_eq!(
            vms,
            ["db-1", "api-1", "api-2", "load-1", "load-1"],
            "{side}"
        );
        // Each workload binds its own `out-<n>`, `db` both; the host writes its own files beside
        // them.
        let binds: Vec<(String, &str, &Utf8Path)> = chunk
            .iter()
            .flat_map(|(launch, _)| writable(launch).map(move |bind| (vm(launch), bind.0, bind.1)))
            .collect();
        let (one, two) = (dir.join("out-1"), dir.join("out-2"));
        assert_eq!(
            binds,
            [
                ("db-1".to_owned(), "/stand/out", one.as_path()),
                ("db-1".to_owned(), "/stand/out-2", two.as_path()),
                ("load-1".to_owned(), "/stand/out", one.as_path()),
                ("load-1".to_owned(), "/stand/out", two.as_path()),
            ],
            "{side}"
        );
        let files = [
            "api-1.log",
            "api-2.log",
            "db-1.log",
            "load-1.log",
            "load-1.pass-1.log",
            "metrics.json",
            "out-1",
            "out-2",
        ];
        assert_eq!(names_in(&dir), files, "{side}");
        for (n, pass) in [(1, first), (2, second)] {
            let out = dir.join(format!("out-{n}"));
            assert_eq!(pass.out_dir, out);
            assert_eq!(ran(&pass.probes), ["orders"]);
            assert_eq!(pass.probes[0].out, out.join("probe.orders.jsonl"));
            assert!(pass.probes[0].out.is_file() && out.join("orders.jsonl").is_file());
            // After the pass, of the long-running services only: the workload's open connections
            // to `api` stay out.
            let ports: Vec<(&str, u64)> = pass
                .ports
                .iter()
                .map(|port| (port.target.as_str(), port.open))
                .collect();
            assert_eq!(ports, [("db:5432", 2), ("db:5432", 2)], "{side} {n}");
        }
        // The cut and the probe of the first pass come at its exit, and its workload is gone
        // before the second starts; the database and the replicas stay, with no restore.
        let (first_load, second_load) = (&chunk[3].1, &chunk[4].1);
        let at = |matches: &dyn Fn(&Call) -> bool| index_of(&journal, matches);
        let cut = at(&|call| matches!(call, Call::Metrics(id) if id == first_load));
        let probe = at(
            &|call| matches!(call, Call::Exec { stdout, .. } if *stdout == one.join("probe.orders.jsonl")),
        );
        let gone = at(&|call| matches!(call, Call::Destroy(id) if id == first_load));
        let again =
            at(&|call| matches!(call, Call::Create { ids, .. } if ids.contains(second_load)));
        let db_gone = at(&|call| matches!(call, Call::Destroy(id) if *id == chunk[0].1));
        assert!(
            cut < probe && probe < gone && gone < again && again < db_gone,
            "{side}"
        );
        let restarts = journal[gone..again]
            .iter()
            .filter(|call| matches!(call, Call::Create { .. } | Call::Stop(_)));
        assert_eq!(restarts.count(), 0, "{side}");
        for (launch, _) in chunk {
            assert_eq!(profile_mounts(launch), (None, false), "{side}");
        }
    }
    // No VM of the run binds a folder that lies in the writable bind of another, where that guest
    // could have swapped it for a link.
    let made = created(&journal);
    for (launch, _) in &made {
        for mount in &launch.mounts {
            let Mount::Bind { host, .. } = mount else {
                continue;
            };
            for (other, _) in &made {
                for (_, around) in writable(other) {
                    let below = host
                        .strip_prefix(around)
                        .is_ok_and(|rest| !rest.as_str().is_empty());
                    assert!(!below, "{host} of {} is in {around}", vm(launch));
                }
            }
        }
    }
    assert_eq!(
        kept_from(&case, &journal),
        [
            ["before", "db", "repeat-3-before"],
            ["head", "db", "repeat-3-head"]
        ]
    );
    assert_clean(&fake, &case.left());
}

/// The guest path and host folder of each writable bind of `launch`.
fn writable(launch: &Launch) -> impl Iterator<Item = (&str, &Utf8Path)> {
    launch.mounts.iter().filter_map(|mount| match mount {
        Mount::Bind {
            host,
            guest,
            read_only: false,
        } => Some((guest.as_str(), host.as_path())),
        _ => None,
    })
}

#[test]
fn a_failed_second_pass_is_a_fact_at_twice_the_timeout() {
    // Head only. By service, the VMs of setup come first, then those of the three measured repeats
    // and the coverage repeat: the second pass makes the fifth and sixth `load` and the ninth and
    // tenth `api`.
    /// `hang` and `ends` of `Hooked`, the failure and the probe folder of the pass that failed.
    struct Failing {
        rules: Vec<Rule>,
        hang: Option<(&'static str, usize)>,
        ends: Vec<End>,
        failure: (Phase, &'static str, Cause),
        out: &'static str,
        loads: usize,
    }
    let cases = [
        // The second workload never ends: the repeat ends at twice `timeout`.
        Failing {
            rules: pass_rules(),
            hang: Some(("load", 6)),
            ends: Vec::new(),
            failure: (Phase::Workload, "load", Cause::Timeout),
            out: "out-2",
            loads: 2,
        },
        // The first workload fails: no second one.
        Failing {
            rules: pass_rules(),
            hang: None,
            ends: vec![end("load", 5, 4, 10 * SECOND)],
            failure: (Phase::Workload, "load", Cause::Exit(4)),
            out: "out-1",
            loads: 1,
        },
        // `api` dies before it is healthy: no workload.
        Failing {
            rules: replaced(pass_rules(), "api", healthy_late()),
            hang: None,
            ends: vec![end("api", 9, 1, SECOND / 2)],
            failure: (Phase::Ready, "api", Cause::Died),
            out: "out-1",
            loads: 0,
        },
    ];
    for Failing {
        rules,
        hang,
        ends,
        failure: (phase, service, cause),
        out,
        loads,
    } in cases
    {
        let case = Case::new(&twice());
        let fake = fake(rules);
        let sandbox = Hooked {
            hang,
            ends,
            ..Hooked::new(&fake)
        };
        let raw = sandbox.run(&case, &case.versions(false)).unwrap();
        assert_eq!(raw.head.len(), 3);
        let pass = raw.second_pass.unwrap();
        assert!(pass.before.is_none());
        let dir = case.run_dir().join("pass2-head");
        let expected = Failure {
            phase,
            service: Some(service.into()),
            repeat: None,
            cause,
            logs: Some(dir.clone()),
            probes: vec![dir.join(out).join("probe.last.jsonl")],
        };
        assert!(
            matches!(&pass.head, PassPair::Failed(failure) if *failure == expected),
            "{:?}",
            pass.head
        );
        let journal = fake.journal();
        let made = created(&journal);
        let first = first_in(&made, "pass2-head");
        let second: Vec<String> = made[first..].iter().map(|(launch, _)| vm(launch)).collect();
        let mut expected = vec!["db-1", "api-1", "api-2"];
        expected.extend(vec!["load-1"; loads]);
        assert_eq!(second, expected, "{service}");
        assert!(dir.join("metrics.json").is_file() && dir.join("db-1.log").is_file());
        if hang.is_some() {
            // The first wait ends a chunk of 1 s after the start, the last one at the deadline.
            let db = &made[first].1;
            assert_eq!(waited(&journal, db), (600 - 1) * SECOND);
        }
        assert_clean(&fake, &case.left());
    }
}
