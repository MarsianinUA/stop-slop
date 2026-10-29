//! The hot functions and the coverage.

use std::collections::BTreeMap;

use profile::Coverage;
use report::{
    Cause, CoverageOutcome, Failure, Flag, Phase, Previous, ProfileOutcome, Report, Role, Verdict,
};
use serde_json::json;

use super::{
    Folder, called, context, instance, metrics, profiled, raw, read, repeats, report, report_of,
    sampled,
};

#[test]
fn a_hot_function_keeps_its_key_when_its_lines_shift() {
    let folder = Folder::new();
    let build = |line, self_ms| [("src/orders.ts", "buildReport", line, 21, self_ms)];
    let earlier = raw(
        &folder,
        profiled(&folder, "previous", &build(41, 900.0)),
        None,
    );
    let previous = report(&earlier);
    let mut head = profiled(&folder, "head", &build(40, 800.0));
    // A second replica adds its time; `rare`, sampled in one repeat of three, has a median of 0.
    for repeat in &mut head {
        let log = folder.log(&format!("api-2-{}", repeat.repeat), "", "");
        let mut second = instance("api", Role::LongRunning, metrics(1000, 100 << 20), log);
        second.replica = 2;
        let mut functions = vec![sampled("src/orders.ts", "buildReport", 40, 100.0)];
        if repeat.repeat == 1 {
            functions.push(sampled("src/orders.ts", "rare", 90, 900.0));
        }
        second.profile = Some(read(functions));
        repeat.instances.push(second);
    }
    let before = profiled(&folder, "before", &build(38, 500.0));
    let raw = raw(&folder, head, Some(before));
    let report = report_of(&raw, &context(&raw, Previous::Read(&previous), &[]), &[]);

    assert_eq!(report.hot.len(), 1, "{:?}", report.hot);
    let hot = &report.hot[0];
    let key = (hot.service.as_str(), hot.file.as_str(), hot.name.as_str());
    assert_eq!(key, ("api", "src/orders.ts", "buildReport"));
    assert_eq!((hot.line, hot.column), (40, 21));
    assert_eq!((hot.self_ms, hot.total_ms), (900.0, 1800.0));
    let before = hot.before.as_ref().map(|before| before.verdict);
    assert_eq!(before, Some(Verdict::Regressed));
    assert_eq!(hot.previous.verdict, Verdict::WithinNoise);
}

#[test]
fn of_equal_names_the_nearest_line_wins_and_under_one_ms_is_left_out() {
    let folder = Folder::new();
    let head = profiled(
        &folder,
        "head",
        &[
            ("src/a.ts", "", 10, 21, 50.0),
            ("src/a.ts", "", 50, 21, 100.0),
            ("src/a.ts", "tiny", 70, 21, 0.5),
            ("src/b.ts", "", 30, 21, 100.0),
            ("src/c.ts", "", 5, 10, 100.0),
            ("src/d.ts", "", 8, 10, 100.0),
        ],
    );
    let before = profiled(
        &folder,
        "before",
        &[
            ("src/a.ts", "", 12, 21, 50.0),
            ("src/a.ts", "", 49, 21, 200.0),
            // Two lines either way: the earlier line wins.
            ("src/b.ts", "", 28, 21, 100.0),
            ("src/b.ts", "", 32, 21, 200.0),
            // The nearest line wins over the nearest column.
            ("src/c.ts", "", 5, 40, 100.0),
            ("src/c.ts", "", 6, 10, 200.0),
            // On one line, the nearest column.
            ("src/d.ts", "", 8, 4, 100.0),
            ("src/d.ts", "", 8, 30, 200.0),
        ],
    );
    let report = report(&raw(&folder, head, Some(before)));

    let found: Vec<(&str, u32, f64, Option<Verdict>)> = report
        .hot
        .iter()
        .map(|hot| {
            let before = hot.before.as_ref().map(|before| before.verdict);
            (hot.file.as_str(), hot.line, hot.self_ms, before)
        })
        .collect();
    let within = Some(Verdict::WithinNoise);
    let expected = [
        ("src/a.ts", 50, 100.0, Some(Verdict::Improved)),
        ("src/b.ts", 30, 100.0, within),
        ("src/c.ts", 5, 100.0, within),
        ("src/d.ts", 8, 100.0, within),
        ("src/a.ts", 10, 50.0, within),
    ];
    assert_eq!(found, expected);
}

#[test]
fn coverage_adds_up_its_services_and_a_failed_repeat_has_none() {
    let folder = Folder::new();
    let mut raw = raw(&folder, repeats(&folder, "head", &[100]), None);
    let api = vec![called("src/f.ts", 1, 1, 2)];
    let worker = vec![
        called("src/f.ts", 1, 1, 3),
        called("src/f.ts", 1, 9, 4),
        called("src/g.ts", 5, 1, 1),
    ];
    raw.coverage = Some(CoverageOutcome::Read(BTreeMap::from([
        ("api".to_owned(), Coverage { called: api }),
        ("worker".to_owned(), Coverage { called: worker }),
    ])));
    let coverage = report(&raw).coverage.expect("coverage");

    assert_eq!(coverage.services, ["api", "worker"]);
    let called = coverage.called.iter();
    let calls: Vec<(&str, u32, u32, u64)> = called
        .map(|called| {
            (
                called.file.as_str(),
                called.line,
                called.column,
                called.calls,
            )
        })
        .collect();
    let expected = [
        ("src/f.ts", 1, 1, 5),
        ("src/f.ts", 1, 9, 4),
        ("src/g.ts", 5, 1, 1),
    ];
    assert_eq!(calls, expected);
    let json = serde_json::to_value(&coverage).unwrap();
    let first = json!({"file": "src/f.ts", "line": 1, "column": 1, "calls": 5});
    assert_eq!(json["called"][0], first);

    raw.coverage = Some(CoverageOutcome::Failed(Failure {
        phase: Phase::Workload,
        service: Some("load".to_owned()),
        repeat: None,
        cause: Cause::Timeout,
        logs: None,
        probes: Vec::new(),
    }));
    let failed = report(&raw);
    assert!(failed.coverage.is_none());
    let unknown = Flag::CoverageUnknown {
        stage: Some(Phase::Workload),
        service: None,
    };
    assert_eq!(unknowns(&failed), [&unknown]);
    let json = serde_json::to_value(&unknown).unwrap();
    let expected = json!({"kind": "coverage-unknown", "stage": "workload", "service": null});
    assert_eq!(json, expected);
    assert_eq!(serde_json::from_value::<Flag>(json).unwrap(), unknown);
}

fn unknowns(report: &Report) -> Vec<&Flag> {
    let flags = report.flags.iter();
    let unknown = |flag: &&Flag| matches!(flag, Flag::CoverageUnknown { .. });
    flags.filter(unknown).collect()
}

#[test]
fn a_service_with_no_call_and_no_profile_leaves_the_coverage_unknown() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[100, 100]);
    head[0].instances[0].profile = Some(read(Vec::new()));
    head[1].instances[0].profile = Some(ProfileOutcome::NoProfile);
    let mut raw = raw(&folder, head, None);
    let worker = Coverage {
        called: vec![called("src/w.ts", 3, 1, 7)],
    };
    raw.coverage = Some(CoverageOutcome::Read(BTreeMap::from([
        ("api".to_owned(), Coverage::default()),
        ("worker".to_owned(), worker),
    ])));
    // `api` may have written no chunk: `stop-slop hot` must not read its files as never run.
    let unseen = report(&raw);
    assert!(unseen.coverage.is_none());
    let unknown = Flag::CoverageUnknown {
        stage: None,
        service: Some("api".to_owned()),
    };
    assert_eq!(unknowns(&unseen), [&unknown]);
    let json = serde_json::to_value(&unknown).unwrap();
    let expected = json!({"kind": "coverage-unknown", "stage": null, "service": "api"});
    assert_eq!(json, expected);

    // A call shows the coverage of `api` was read, whatever its measured repeats wrote.
    if let Some(CoverageOutcome::Read(services)) = &mut raw.coverage {
        let api = services.get_mut("api").expect("api");
        api.called.push(called("src/a.ts", 1, 1, 1));
    }
    let called_once = report(&raw);
    assert!(called_once.coverage.is_some());
    assert!(unknowns(&called_once).is_empty());

    // With a profile in every measured repeat, no call is a fact: no function of `api` ran.
    raw.head[1].instances[0].profile = Some(read(Vec::new()));
    if let Some(CoverageOutcome::Read(services)) = &mut raw.coverage {
        services.insert("api".to_owned(), Coverage::default());
    }
    let report = report(&raw);
    let coverage = report.coverage.as_ref().expect("coverage");
    assert_eq!(coverage.services, ["api", "worker"]);
    assert_eq!(coverage.called.len(), 1);
    assert!(unknowns(&report).is_empty());
}

#[test]
fn a_coverage_repeat_of_no_service_leaves_the_coverage_unknown() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[100]);
    head[0].instances[0].profile = Some(read(Vec::new()));
    let mut raw = raw(&folder, head, None);
    raw.coverage = Some(CoverageOutcome::Read(BTreeMap::new()));
    let report = report(&raw);

    assert!(report.coverage.is_none());
    let unknown = Flag::CoverageUnknown {
        stage: None,
        service: None,
    };
    assert_eq!(unknowns(&report), [&unknown]);
}
