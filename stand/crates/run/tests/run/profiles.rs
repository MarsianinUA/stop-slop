//! The profiles of the measured repeats and the coverage repeat.

use std::fs;
use std::time::Duration;

use profile::ProfileError;
use report::{BeforeOutcome, Cause, CoverageOutcome, Failure, Phase, ProfileOutcome};
use run::{RunError, Side, StandFault};
use sandbox::{Call, Rule, Script, Seed};

use super::{
    Case, EXAMPLE, Hooked, NO_PREPARE, SECOND, assert_clean, created, example_rules, execs, exits,
    fake, index_of, no_prepare_rules, out_of, profile_mounts, replaced, setup_snapshots, stops, vm,
    waited, with, workload_folders,
};

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
