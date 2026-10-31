//! The flags that the raw outputs and the context give.

use camino::Utf8Path;
use profile::{ModuleTime, Samples};
use report::{
    BeforeOutcome, Bound, Cause, CoverageOutcome, DeadReason, Declared, Failure, FaultOutcome,
    FaultRepeat, Flag, Phase, Previous, ProfileOutcome, Role, Side,
};
use sandbox::{ExecOutcome, Unexposed};
use serde_json::json;

use super::{
    Folder, GIB, context, declared, instance, killed, metrics, port, probe_ran, raw, raw_of, read,
    repeats, report, report_of, sampled, setup_side,
};

#[test]
fn each_fact_of_the_raw_outputs_gives_its_flag() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[100]);
    let repeat = &mut head[0];
    let out = repeat.out_dir.clone();
    let api = &mut repeat.instances[0];
    api.profile = Some(ProfileOutcome::NoProfile);
    api.stopped = killed();
    api.metrics.unexposed.push(Unexposed {
        name: "cache".to_owned(),
        attempts: 4,
    });
    // The guest's `net` lists every peer without `expose`, the uncalled ones too.
    api.metrics.unexposed.push(Unexposed {
        name: "load".to_owned(),
        attempts: 0,
    });
    let worker_log = folder.log("worker", "", "");
    let mut worker = instance(
        "worker",
        Role::LongRunning,
        metrics(10, 10 << 20),
        worker_log,
    );
    // 30 of the 100 ms of TypeScript found no map; the 300 ms of JavaScript are not TypeScript.
    worker.profile = Some(ProfileOutcome::Read(Samples {
        total_ms: 400.0,
        local_ms: 400.0,
        unmapped_ms: 30.0,
        processes: 3,
        lost_processes: 2,
        functions: vec![
            sampled("src/worker.ts", "work", 3, 50.0),
            sampled("scripts/seed.js", "seed", 1, 300.0),
        ],
        modules: vec![ModuleTime {
            file: "src/main.ts".into(),
            self_ms: 20.0,
        }],
    }));
    repeat.instances.push(worker);
    let load = &mut repeat.instances[1].metrics;
    load.memory_peak = GIB / 20 * 19;
    load.ports[0].connections = 300;
    load.ports[0].drops = 5;
    let orders = probe_ran("orders", ExecOutcome::Exited(2), &out);
    repeat.probes.push(orders);
    let mut raw = raw(&folder, head, None);
    let setup_out = raw.setup.head.out_dir.clone();
    let seed = probe_ran("seed", ExecOutcome::TimedOut, &setup_out);
    raw.setup.head.probes.push(seed);
    raw.coverage = Some(CoverageOutcome::Failed(Failure {
        phase: Phase::Ready,
        service: Some("api".to_owned()),
        repeat: None,
        cause: Cause::Unhealthy,
        logs: None,
        probes: Vec::new(),
    }));
    let report = report(&raw);

    let expected = [
        Flag::LowRepeats { samples: 1 },
        Flag::NoProfile {
            service: "api".to_owned(),
        },
        Flag::LostProcesses {
            service: "worker".to_owned(),
            count: 2,
        },
        Flag::UnmappedTime {
            service: "worker".to_owned(),
            share: 0.3,
        },
        Flag::MemoryNearLimit {
            service: "load".to_owned(),
            peak: GIB / 20 * 19,
            limit: GIB,
        },
        Flag::NetworkLimitHit {
            repeat: 1,
            target: "api:3000".to_owned(),
            drops: 5,
        },
        Flag::MissingExpose {
            name: "cache".to_owned(),
            attempts: 4,
        },
        Flag::CoverageUnknown {
            stage: Some(Phase::Ready),
            service: None,
        },
        // Both ran and wrote no `probe.<name>.jsonl`.
        Flag::DeadProbe {
            probe: "orders".to_owned(),
            reason: DeadReason::NoRecords,
        },
        Flag::DeadProbe {
            probe: "seed".to_owned(),
            reason: DeadReason::NoRecords,
        },
        Flag::ProbeFailed {
            probe: "seed".to_owned(),
            side: Side::Head,
            repeat: None,
            code: None,
        },
        Flag::ProbeFailed {
            probe: "orders".to_owned(),
            side: Side::Head,
            repeat: Some(1),
            code: Some(2),
        },
        Flag::KilledAtStop {
            service: "api".to_owned(),
            side: Side::Head,
            repeat: 1,
        },
    ];
    assert_eq!(report.flags, expected);
}

#[test]
fn network_limit_hit_takes_drops_and_256_connections_to_the_target_over_all_guests() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[100, 100, 100]);
    let mut before = repeats(&folder, "before", &[100, 100, 100]);
    // 200 from the workload and 56 from `api` itself reach `api:3000` in the first repeat.
    head[0].instances[1].metrics.ports[0].connections = 200;
    let mut back = port("api:3000", 56);
    back.drops = 1;
    head[0].instances[0].metrics.ports.push(back);
    // Drops under 256 connections are a target that was not ready: they add to no flag.
    let calls = [(255, 3), (300, 0)];
    for (repeat, (connections, drops)) in head[1..].iter_mut().zip(calls) {
        let port = &mut repeat.instances[1].metrics.ports[0];
        (port.connections, port.drops) = (connections, drops);
    }
    let calls = [(300, 2), (300, 4), (100, 5)];
    for (repeat, (connections, drops)) in before.iter_mut().zip(calls) {
        let port = &mut repeat.instances[1].metrics.ports[0];
        (port.connections, port.drops) = (connections, drops);
    }
    let report = report(&raw(&folder, head, Some(before)));

    let flags = report.flags.iter();
    let hits: Vec<&Flag> = flags
        .filter(|flag| matches!(flag, Flag::NetworkLimitHit { .. }))
        .collect();
    let hit = |repeat, drops| Flag::NetworkLimitHit {
        repeat,
        target: "api:3000".to_owned(),
        drops,
    };
    assert_eq!(hits, [&hit(1, 3), &hit(2, 4)]);
}

#[test]
fn a_target_with_two_replicas_hits_the_limit_from_512_connections() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[100, 100]);
    for (repeat, connections) in head.iter_mut().zip([511, 512]) {
        let log = folder.log(&format!("api-2-{}", repeat.repeat), "", "");
        let mut second = instance("api", Role::LongRunning, metrics(1000, 100 << 20), log);
        second.replica = 2;
        repeat.instances.push(second);
        let port = &mut repeat.instances[1].metrics.ports[0];
        (port.connections, port.drops) = (connections, 2);
    }
    let report = report(&raw(&folder, head, None));

    let flags = report.flags.iter();
    let hits: Vec<&Flag> = flags
        .filter(|flag| matches!(flag, Flag::NetworkLimitHit { .. }))
        .collect();
    let hit = Flag::NetworkLimitHit {
        repeat: 2,
        target: "api:3000".to_owned(),
        drops: 2,
    };
    assert_eq!(hits, [&hit]);
}

#[test]
fn a_run_without_a_profiled_service_is_unprofiled() {
    let folder = Folder::new();
    let plain = report(&raw(&folder, repeats(&folder, "a", &[100]), None));
    assert!(plain.flags.contains(&Flag::Unprofiled));
    let json = serde_json::to_value(&Flag::Unprofiled).unwrap();
    assert_eq!(json, json!({"kind": "unprofiled"}));
    assert_eq!(
        serde_json::from_value::<Flag>(json).unwrap(),
        Flag::Unprofiled
    );

    let mut head = repeats(&folder, "b", &[100]);
    head[0].instances[0].profile = Some(read(Vec::new()));
    let profiled = report(&raw(&folder, head, None));
    assert!(!profiled.flags.contains(&Flag::Unprofiled));
}

#[test]
fn previous_unreadable_and_recipe_changed_come_from_the_context() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[100]);
    head[0].instances[0].stopped = killed();
    let raw = raw(&folder, head, None);
    let unreadable = report_of(&raw, &context(&raw, Previous::Unreadable, &[]), &[]);
    // Found after the facts of `Raw`, listed in the order of the kinds.
    let expected = [
        Flag::LowRepeats { samples: 1 },
        Flag::Unprofiled,
        Flag::PreviousUnreadable,
        Flag::KilledAtStop {
            service: "api".to_owned(),
            side: Side::Head,
            repeat: 1,
        },
    ];
    assert_eq!(unreadable.flags, expected);

    let mut other = raw_of(&folder, "other");
    other.digest = "0".repeat(64);
    let changed = report(&other);
    let flagged = report_of(&raw, &context(&raw, Previous::Read(&changed), &[]), &[]);
    assert!(flagged.flags.contains(&Flag::RecipeChanged));
    let same = report(&raw_of(&folder, "same"));
    let calm = report_of(&raw, &context(&raw, Previous::Read(&same), &[]), &[]);
    assert!(!calm.flags.contains(&Flag::RecipeChanged));
}

#[test]
fn a_kill_at_the_stop_of_a_fault_repeat_is_no_flag() {
    let folder = Folder::new();
    let mut raw = raw(&folder, repeats(&folder, "head", &[100, 100, 100]), None);
    let held_log = folder.log("fault-db", "", "");
    let mut held = instance("db", Role::LongRunning, metrics(10, 10 << 20), held_log);
    held.stopped = killed();
    raw.fault = Some(FaultOutcome {
        target: "db:5432".to_owned(),
        mode: "down".to_owned(),
        head: vec![FaultRepeat {
            repeat: 1,
            workload: Some(0),
            exited: Vec::new(),
            failure: None,
            instances: vec![held],
        }],
        before: None,
    });
    let report = report(&raw);

    let mut flags = report.flags.iter();
    assert!(!flags.any(|flag| matches!(flag, Flag::KilledAtStop { .. })));
}

#[test]
fn a_wide_spread_is_high_noise_and_a_far_first_repeat_an_outlier() {
    let folder = Folder::new();
    let wall = || "stand.wall".to_owned();
    let noisy = report(&raw(&folder, repeats(&folder, "a", &[100, 125, 80]), None));
    assert!(noisy.flags.contains(&Flag::HighNoise { measure: wall() }));
    assert!(
        !noisy
            .flags
            .contains(&Flag::FirstRepeatOutlier { measure: wall() })
    );

    // The others lie within 1% of 100.5; the first is 19% above them, and the spread stays
    // under 20%.
    let warm = report(&raw(&folder, repeats(&folder, "b", &[120, 100, 101]), None));
    assert!(
        warm.flags
            .contains(&Flag::FirstRepeatOutlier { measure: wall() })
    );
    assert!(!warm.flags.contains(&Flag::HighNoise { measure: wall() }));
}

#[test]
fn with_before_high_noise_is_only_of_a_measure_the_line_shows() {
    let folder = Folder::new();
    let wall = Flag::HighNoise {
        measure: "stand.wall".to_owned(),
    };
    let compared = |name: &str, head: &[u64], declared: Vec<Declared>| {
        let before = Some(repeats(
            &folder,
            &format!("{name}-before"),
            &[100, 100, 100],
        ));
        let raw = raw(
            &folder,
            repeats(&folder, &format!("{name}-head"), head),
            before,
        );
        let mut context = context(&raw, Previous::None, &[]);
        context.declared = declared;
        let report = report_of(&raw, &context, &[]);
        let line = report.line(Utf8Path::new("run"));
        let shown = line.measures.iter().any(|known| known.name == "stand.wall");
        (
            shown,
            report.flags.contains(&wall),
            line.flags.contains(&wall),
        )
    };

    // A spread of 45% hides the shift, and without an entry the line leaves it quiet.
    let noisy = [100, 125, 80];
    assert_eq!(compared("quiet", &noisy, Vec::new()), (false, false, false));
    // Within a bound of +50% it is `met`, and its entry puts it in the line.
    let entry = vec![declared("stand.wall", Bound::UpTo(50))];
    assert_eq!(compared("declared", &noisy, entry), (true, true, true));
    // A doubling shows through the noise: a `surprise` the line shows.
    let doubled = [200, 250, 160];
    assert_eq!(
        compared("shifted", &doubled, Vec::new()),
        (true, true, true)
    );
}

#[test]
fn low_repeats_counts_the_pooled_samples() {
    let folder = Folder::new();
    let alone = report(&raw(&folder, repeats(&folder, "a", &[100]), None));
    assert!(alone.flags.contains(&Flag::LowRepeats { samples: 1 }));

    let pool = vec![
        report(&raw(&folder, repeats(&folder, "b", &[100]), None)),
        report(&raw(&folder, repeats(&folder, "c", &[100]), None)),
    ];
    let raw = raw(&folder, repeats(&folder, "head", &[100]), None);
    let pooled = report_of(&raw, &context(&raw, Previous::None, &pool), &[]);
    let mut flags = pooled.flags.iter();
    assert!(!flags.any(|flag| matches!(flag, Flag::LowRepeats { .. })));
}

#[test]
fn a_failed_before_is_a_flag_with_its_stage() {
    let folder = Folder::new();
    let mut raw = raw(&folder, repeats(&folder, "head", &[100, 100, 100]), None);
    raw.setup.before = Some(setup_side(&folder, "before", &[]));
    raw.before = Some(BeforeOutcome::Failed(Failure {
        phase: Phase::Workload,
        service: Some("load".to_owned()),
        repeat: Some(2),
        cause: Cause::Timeout,
        logs: None,
        probes: Vec::new(),
    }));
    let report = report(&raw);

    let flag = Flag::BeforeFailed {
        stage: Phase::Workload,
    };
    assert!(report.flags.contains(&flag));
    let json = serde_json::to_value(&flag).unwrap();
    assert_eq!(json, json!({"kind": "before-failed", "stage": "workload"}));
    assert_eq!(serde_json::from_value::<Flag>(json).unwrap(), flag);
}
