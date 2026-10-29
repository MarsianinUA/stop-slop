//! The quiet line and the summary.

use std::collections::BTreeMap;
use std::time::Duration;

use camino::Utf8Path;
use profile::Coverage;
use report::{
    BeforeOutcome, Cause, CoverageOutcome, Direction, Failure, Flag, Line, Next, Phase, Previous,
    ProfileOutcome, Report, Role, Side, Unit, Verdict,
};
use sandbox::{ExecOutcome, Unexposed};

use super::{
    Folder, PREVIOUS, RUN, behaviour, context, failure_of, instance, line_files, measure, metrics,
    names, probe_ran, profiled, raw, read, repeats, report, report_of, sampled, setup_side,
    slashed, verdicts, with_files,
};

fn first_line(report: &Report) -> String {
    let summary = report.summary().to_string();
    summary.lines().next().unwrap_or_default().to_owned()
}

#[test]
fn the_quiet_line_holds_only_the_exceptions_and_reads_no_file() {
    let folder = Folder::new();
    let many: String = (1..=5)
        .map(|total| format!("{{\"total\":{total}}}\n"))
        .collect();
    let none: String = (1..=5).map(|_| "{\"total\":0}\n").collect();
    // `broken.jsonl` has no diff, but a flag names it.
    let head = [
        ("out.jsonl", many.as_str()),
        ("same.jsonl", "{}\n"),
        ("broken.jsonl", "{}\nnot json\n"),
    ];
    let before = [
        ("out.jsonl", none.as_str()),
        ("same.jsonl", "{}\n"),
        ("broken.jsonl", "{}\n"),
    ];
    let head = with_files(&folder, "head", &[&head]);
    let before = with_files(&folder, "before", &[&before]);
    let mut raw = raw(&folder, head, Some(before));
    raw.head[0].wall = Duration::from_millis(2000);
    let report = report(&raw);
    let dir = Utf8Path::new("nowhere").join("run");
    let line = report.line(&dir);

    let names: Vec<&str> = line
        .measures
        .iter()
        .map(|measure| measure.name.as_str())
        .collect();
    assert_eq!(names, ["stand.wall"]);
    assert_eq!(
        line_files(&line),
        [("broken.jsonl", 0, 0), ("out.jsonl", 5, 3)]
    );
    let out = &line.behaviour.as_ref().expect("files")[1];
    assert_eq!(out.sample, behaviour(&report, "out.jsonl").diffs[..3]);
    assert_eq!(out.path, dir.join("diff").join("out.jsonl.json"));
    // Every measure but `stand.wall`, and `same.jsonl`.
    assert_eq!(line.quiet as usize, report.measures.len() - 1 + 1);
    assert_eq!(line.run.as_deref(), Some(RUN));
    assert_eq!(line.report, Some(dir.join("report.json")));
    assert_eq!(line.flags, report.flags);
    assert!(line.failure.is_none() && line.next.is_empty());
    let json = serde_json::to_value(&line).unwrap();
    assert!(
        json.get("next").is_none(),
        "cli prints `next`, not the line"
    );
    assert_eq!(json["report"], "nowhere/run/report.json");
    assert_eq!(
        json["behaviour"][1]["path"],
        "nowhere/run/diff/out.jsonl.json"
    );
    assert_eq!(json["unmeasurable"], 0);
    assert!(json["measures"][0].get("samples").is_none());
}

#[test]
fn without_before_a_shift_against_the_previous_run_is_an_exception() {
    let folder = Folder::new();
    let previous = report(&raw(
        &folder,
        repeats(&folder, "previous", &[100, 100, 100]),
        None,
    ));
    let raw = raw(&folder, repeats(&folder, "head", &[200, 200, 200]), None);
    let report = report_of(&raw, &context(&raw, Previous::Read(&previous), &[]), &[]);

    let line = report.line(Utf8Path::new("run"));
    let names: Vec<&str> = line
        .measures
        .iter()
        .map(|measure| measure.name.as_str())
        .collect();
    assert_eq!(names, ["stand.wall"]);
    assert_eq!(line.measures[0].previous.verdict, Verdict::Regressed);
    assert!(line.measures[0].before.is_none() && line.behaviour.is_none());
    assert_eq!(line.quiet as usize, report.measures.len() - 1);
}

#[test]
fn a_failed_line_carries_the_facts_so_far_and_its_next_steps() {
    // The head fails in its second repeat: `schema` after setup and `orders` after the first
    // workload exited with a code, and `rescue` ran on the failure.
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[100, 100]);
    let first = head[0].out_dir.clone();
    head[0]
        .probes
        .push(probe_ran("orders", ExecOutcome::Exited(2), &first));
    let failed = head[1].out_dir.clone();
    let rescue = probe_ran("rescue", ExecOutcome::Exited(3), &failed);
    head[1].probes.push(rescue.clone());
    head[1].instances[0].metrics.unexposed.push(Unexposed {
        name: "cache".to_owned(),
        attempts: 2,
    });
    let mut raw = raw(&folder, head, None);
    let setup_out = raw.setup.head.out_dir.clone();
    let schema = probe_ran("schema", ExecOutcome::Exited(1), &setup_out);
    raw.setup.head.probes.push(schema);
    let logs = folder.path().join("repeat-2-head");
    let failure = Failure {
        phase: Phase::Workload,
        service: Some("load".to_owned()),
        repeat: Some(2),
        cause: Cause::Exit(1),
        logs: Some(logs.clone()),
        probes: vec![rescue.out.clone()],
    };
    let next = vec![Next::Inspect {
        run: RUN.to_owned(),
        service: "db".to_owned(),
    }];
    let line = Line::failed(failure.clone(), Some(&raw), next.clone());

    let probe_failed = |probe: &str, repeat, code| Flag::ProbeFailed {
        probe: probe.to_owned(),
        side: Side::Head,
        repeat,
        code,
    };
    // The failed second repeat gave no sample.
    let expected = [
        Flag::LowRepeats { samples: 1 },
        Flag::Unprofiled,
        Flag::MissingExpose {
            name: "cache".to_owned(),
            attempts: 2,
        },
        probe_failed("schema", None, Some(1)),
        probe_failed("orders", Some(1), Some(2)),
    ];
    assert_eq!(line.flags, expected);
    assert_eq!(line.failure.as_ref(), Some(&failure));
    assert_eq!(line.next, next);
    assert!(line.measures.is_empty() && line.behaviour.is_none());
    assert!(line.run.is_none() && line.report.is_none());
    let json = serde_json::to_value(&line).unwrap();
    assert_eq!(json["failure"]["logs"], slashed(&logs));
    assert!(json["run"].is_null() && json["report"].is_null());
    let text = line.summary().to_string();
    assert!(
        text.starts_with("failure workload load repeat 2: exit-1\n"),
        "{text}"
    );
    assert!(
        text.contains(&format!("\nlogs {}\n", slashed(&logs))),
        "{text}"
    );
    assert!(
        text.contains(&format!("\nprobes {}\n", slashed(&rescue.out))),
        "{text}"
    );

    // A step after `run` failed: both repeats passed, no probe ran on the failure, and `schema`
    // keeps its flag although the failure has no repeat either.
    raw.head[1].probes.clear();
    let after_run = Failure {
        phase: Phase::Report,
        service: None,
        repeat: None,
        cause: Cause::Error("repeat-1-head/out/out.jsonl: not a plain file".to_owned()),
        logs: None,
        probes: Vec::new(),
    };
    let line = Line::failed(after_run, Some(&raw), Vec::new());
    let mut expected = expected.to_vec();
    expected[0] = Flag::LowRepeats { samples: 2 };
    assert_eq!(line.flags, expected);

    // A failed setup: its probes ran on the failure too, and no repeat ran.
    raw.head.clear();
    let rescue = probe_ran("rescue", ExecOutcome::Exited(3), &setup_out);
    raw.setup.head.probes = vec![rescue.clone()];
    let failure = Failure {
        phase: Phase::Setup,
        service: Some("db".to_owned()),
        repeat: None,
        cause: Cause::Unhealthy,
        logs: Some(folder.path().join("setup-head")),
        probes: vec![rescue.out],
    };
    let line = Line::failed(failure.clone(), Some(&raw), Vec::new());
    assert!(line.flags.is_empty(), "{:?}", line.flags);

    let refused = Line::failed(failure, None, vec![Next::Init]);
    assert!(refused.flags.is_empty());
    assert_eq!(refused.next, [Next::Init]);
}

#[test]
fn the_line_and_the_summary_print_the_same_tokens() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[200, 200, 200]);
    head[0].instances[0].metrics.unexposed.push(Unexposed {
        name: "cache".to_owned(),
        attempts: 2,
    });
    let before = repeats(&folder, "before", &[100, 100, 100]);
    let raw = raw(&folder, head, Some(before));
    let report = report(&raw);
    let line = report.line(Utf8Path::new("run"));
    let texts = [report.summary().to_string(), line.summary().to_string()];

    let json = serde_json::to_value(&line).unwrap();
    let mut tokens = vec![
        "stand.wall 200 ms before 100 regressed",
        "missing-expose attempts 2 name cache",
    ];
    let kinds = json["flags"].as_array().unwrap().iter();
    tokens.extend(kinds.map(|flag| flag["kind"].as_str().unwrap()));
    let quiet = format!("\nquiet {}\n", line.quiet);
    tokens.push(&quiet);
    for text in &texts {
        for token in &tokens {
            assert!(text.contains(token), "{token} not in\n{text}");
        }
    }
    assert!(
        texts[1].ends_with("\nreport run/report.json\n"),
        "{}",
        texts[1]
    );
    assert!(!texts[0].contains("report.json"), "{}", texts[0]);
}

#[test]
fn the_summary_opens_with_no_comparison_and_coverage_unknown() {
    let folder = Folder::new();
    // Without a profiled service there is no coverage to miss: `unprofiled` stays a flag line.
    let plain = report(&raw(&folder, repeats(&folder, "plain", &[100]), None));
    assert_eq!(first_line(&plain), format!("run {RUN}"));
    let summary = plain.summary().to_string();
    assert!(
        summary.lines().any(|line| line == "unprofiled"),
        "{summary}"
    );

    let mut raw = raw(&folder, repeats(&folder, "head", &[100, 100, 100]), None);
    raw.setup.before = Some(setup_side(&folder, "before", &[]));
    raw.before = Some(BeforeOutcome::Failed(failure_of(Phase::Setup)));
    raw.coverage = Some(CoverageOutcome::Failed(failure_of(Phase::Workload)));
    let summary = report(&raw).summary().to_string();
    let first: Vec<&str> = summary.lines().take(3).collect();
    let opening = [
        "NO COMPARISON: before failed at setup",
        "coverage unknown: repeat failed at workload",
    ];
    let run = format!("run {RUN}");
    assert_eq!(first, [opening[0], opening[1], run.as_str()]);
    let line = Line::failed(failure_of(Phase::Workload), Some(&raw), Vec::new());
    let text = line.summary().to_string();
    let opened = format!("{}\n{}\n", opening[0], opening[1]);
    assert!(text.starts_with(&opened), "{text}");

    // `api` wrote no profile in a measured repeat, and the coverage repeat saw no call of it.
    raw.before = None;
    raw.setup.before = None;
    raw.head[0].instances[0].profile = Some(ProfileOutcome::NoProfile);
    let services = BTreeMap::from([("api".to_owned(), Coverage::default())]);
    raw.coverage = Some(CoverageOutcome::Read(services));
    let unseen = "coverage unknown: api left no coverage";
    assert_eq!(first_line(&report(&raw)), unseen);
    raw.coverage = Some(CoverageOutcome::Read(BTreeMap::new()));
    let empty = "coverage unknown: no service in the coverage repeat";
    assert_eq!(first_line(&report(&raw)), empty);
}

#[test]
fn the_summary_names_the_setup_origin_and_the_line_does_not() {
    let folder = Folder::new();
    let raw = raw(
        &folder,
        repeats(&folder, "head", &[100]),
        Some(repeats(&folder, "before", &[100])),
    );
    let mut context = context(&raw, Previous::None, &[]);
    context.setup_from.before = Some(PREVIOUS.to_owned());
    let report = report_of(&raw, &context, &[]);

    let summary = report.summary().to_string();
    let setup = format!("setup head built  before cached {PREVIOUS}");
    assert!(summary.lines().any(|line| line == setup), "{summary}");
    let line = serde_json::to_string(&report.line(Utf8Path::new("run"))).unwrap();
    assert!(
        !line.contains("cached") && !line.contains("setup"),
        "{line}"
    );
}

#[test]
fn the_self_time_of_changed_files_is_the_own_cost_and_the_rest_collateral() {
    let folder = Folder::new();
    let changed_head = [
        ("src/orders.ts", "build", 10, 21, 300.0),
        ("src/orders.ts", "total", 30, 5, 20.5),
        ("src/util.ts", "format", 3, 5, 50.0),
    ];
    let changed_before = [
        ("src/orders.ts", "build", 10, 21, 100.0),
        ("src/util.ts", "format", 3, 5, 50.0),
    ];
    let mut head = profiled(&folder, "head", &changed_head);
    // A second replica adds to the repeat; a repeat where one of them lost its profile has no
    // sample.
    let replica = |name: &str, profile| {
        let log = folder.log(name, "", "");
        let mut replica = instance("api", Role::LongRunning, metrics(1000, 100 << 20), log);
        replica.replica = 2;
        replica.profile = Some(profile);
        replica
    };
    let read_two = read(vec![sampled("src/orders.ts", "build", 10, 10.0)]);
    head[0].instances.push(replica("api-2", read_two));
    head[1]
        .instances
        .push(replica("api-2-lost", ProfileOutcome::NoProfile));
    let raw = raw(
        &folder,
        head,
        Some(profiled(&folder, "before", &changed_before)),
    );
    let mut compared = context(&raw, Previous::None, &[]);
    compared.changed = Some(["src/orders.ts".to_owned()].into());
    let report = report_of(&raw, &compared, &[]);

    let owned = measure(&report, "stand.hot-owned");
    assert_eq!(owned.samples, [330.5, 320.5]);
    let before = owned.before.as_ref().expect("a before");
    assert_eq!(before.samples, [100.0, 100.0, 100.0]);
    assert_eq!((owned.unit, owned.direction), (Unit::Ms, Direction::Lower));
    assert_eq!(verdicts(owned).0, Some(Verdict::Regressed));
    let collateral = measure(&report, "stand.hot-collateral");
    assert_eq!(collateral.samples, [50.0, 50.0]);
    assert_eq!(verdicts(collateral).0, Some(Verdict::WithinNoise));
    assert_eq!(
        &names(&report)[..3],
        ["stand.wall", "stand.hot-owned", "stand.hot-collateral"]
    );

    // Neither without `--before`, nor without a profile.
    let plain = self::raw(&folder, profiled(&folder, "plain", &changed_head), None);
    let mut explored = context(&plain, Previous::None, &[]);
    explored.changed = compared.changed.clone();
    let unprofiled = self::raw(
        &folder,
        repeats(&folder, "bare", &[100]),
        Some(repeats(&folder, "bare-before", &[100])),
    );
    let mut bare = context(&unprofiled, Previous::None, &[]);
    bare.changed = compared.changed.clone();
    // Nor when no repeat of the head read a profile: there is nothing to compare, not a 0.
    let mut lost = profiled(&folder, "lost", &changed_head);
    for repeat in &mut lost {
        repeat.instances[0].profile = Some(ProfileOutcome::NoProfile);
    }
    let lost = self::raw(
        &folder,
        lost,
        Some(profiled(&folder, "lost-before", &changed_before)),
    );
    let mut unseen = context(&lost, Previous::None, &[]);
    unseen.changed = compared.changed.clone();
    for report in [
        report_of(&plain, &explored, &[]),
        report_of(&unprofiled, &bare, &[]),
        report_of(&lost, &unseen, &[]),
    ] {
        let names = names(&report);
        assert!(
            !names.iter().any(|name| name.starts_with("stand.hot")),
            "{names:?}"
        );
    }
}
