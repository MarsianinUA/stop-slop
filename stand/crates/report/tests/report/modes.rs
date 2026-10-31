//! The fault repeats and the second pass.

use std::collections::BTreeSet;
use std::fs;

use camino::Utf8Path;
use report::{
    At, BadResult, CategoryChange, Cause, Diff, Extra, FaultJson, FaultOutcome, Flag, PassFile,
    PassPair, PassState, Phase, Previous, ReportError, SecondPassOutcome, Side, load, read_outputs,
};
use serde_json::{Value, json};

use super::{
    Folder, context, failure_of, faulted, fixture_modes, fixture_raw, line_files, pass, pass_file,
    passed, probe_rule, raw, raw_of, repeats, report, report_of, slashed, with_files, written,
};

fn change(category: &str, before: &str, head: &str) -> CategoryChange {
    CategoryChange {
        category: category.to_owned(),
        before: before.to_owned(),
        head: head.to_owned(),
    }
}

#[test]
fn the_fault_repeats_compare_categories_and_a_disagreeing_before_is_unknown() {
    let folder = Folder::new();
    let before = Some(repeats(&folder, "before", &[100, 100, 100]));
    let mut raw = raw(&folder, repeats(&folder, "head", &[100, 100, 100]), before);
    let calm = |repeat| faulted(&folder, repeat, Ok(0), &[], (300, [0, 290, 0, 0, 10]), 2);
    let crashed =
        |repeat, ended| faulted(&folder, repeat, ended, &["api"], (40, [0, 40, 0, 0, 0]), 12);
    let timeout = Err((Phase::Workload, Cause::Timeout));
    raw.fault = Some(FaultOutcome {
        target: "db:5432".to_owned(),
        mode: "down".to_owned(),
        head: vec![crashed(1, Ok(1)), crashed(2, timeout.clone())],
        before: Some(vec![calm(1), calm(2)]),
    });
    let report = report(&raw);

    let fault = report.fault.as_ref().expect("a fault");
    let json = serde_json::to_value(fault).unwrap();
    let before = json!([
        {"category": "workload", "values": ["exit-0", "exit-0"]},
        {"category": "api state", "values": ["running", "running"]},
        {"category": "db state", "values": ["running", "running"]},
        {"category": "api:3000 attempts", "values": [">10", ">10"]},
        {"category": "db:5432 attempts", "values": ["2-3", "2-3"]},
        {"category": "api:3000 responses", "values": ["2xx 5xx", "2xx 5xx"]},
    ]);
    assert_eq!(json["categories"]["before"], before);
    assert_eq!(
        json["categories"]["head"][0]["values"],
        json!(["exit-1", "timeout"])
    );
    let changed = [
        change("workload", "exit-0", "exit-1"),
        change("workload", "exit-0", "timeout"),
        change("api state", "running", "exited"),
        change("db:5432 attempts", "2-3", ">10"),
        change("api:3000 responses", "2xx 5xx", "2xx"),
    ];
    assert_eq!(fault.changed, changed);
    assert!(!fault.unknown());
    let line = report.line(Utf8Path::new("run"));
    let shown = line.fault.as_ref().expect("a fault in the line");
    assert_eq!(
        (shown.target.as_str(), shown.mode.as_str()),
        ("db:5432", "down")
    );
    assert_eq!(shown.changed, changed);
    assert!(shown.unknown.is_empty());
    let summary = report.summary().to_string();
    for text in [
        "fault db:5432 down",
        "fault api state before running head exited",
    ] {
        assert!(
            summary.lines().any(|line| line == text),
            "{text} not in\n{summary}"
        );
    }

    // "Before" disagrees on its workload, and the head's second repeat never reached the fault.
    let fault = raw.fault.as_mut().expect("a fault");
    let exited = faulted(&folder, 2, Ok(1), &[], (300, [0, 290, 0, 0, 10]), 2);
    fault.before = Some(vec![calm(1), exited]);
    let unready = Err((Phase::Ready, Cause::Unhealthy));
    fault.head[1] = faulted(&folder, 2, unready, &["db"], (0, [0; 5]), 0);
    let report = self::report(&raw);
    let fault = report.fault.as_ref().expect("a fault");
    let head = &fault.categories.head;
    assert_eq!(head[0].values, ["exit-1", "unknown"]);
    assert_eq!(head[2].values, ["running", "unknown"], "{head:?}");
    let changed = [
        change("api state", "running", "exited"),
        change("db:5432 attempts", "2-3", ">10"),
        change("api:3000 responses", "2xx 5xx", "2xx"),
    ];
    assert_eq!(fault.changed, changed);
    assert!(fault.unknown());
    let line = report.line(Utf8Path::new("run"));
    assert_eq!(line.fault.as_ref().expect("a fault").unknown, ["workload"]);
    let summary = report.summary().to_string();
    assert!(
        summary.lines().any(|line| line == "fault workload unknown"),
        "{summary}"
    );
    // Neither repeat of "before" reached the fault: nothing compares, nothing changed.
    let mut unready = raw.fault.take().expect("a fault");
    let ready = || {
        faulted(
            &folder,
            1,
            Err((Phase::Ready, Cause::Unhealthy)),
            &[],
            (0, [0; 5]),
            0,
        )
    };
    unready.before = Some(vec![ready(), ready()]);
    raw.fault = Some(unready);
    let blind = self::report(&raw);
    let fault_of = blind.fault.as_ref().expect("a fault");
    assert!(fault_of.changed.is_empty(), "{:?}", fault_of.changed);
    let unknown = blind
        .line(Utf8Path::new("run"))
        .fault
        .expect("a fault")
        .unknown;
    assert_eq!(unknown.len(), fault_of.categories.head.len());
    // A loaded report tells it the same: `cli` reads it there for the pool.
    let loaded: FaultJson = serde_json::from_value(serde_json::to_value(fault).unwrap()).unwrap();
    assert!(loaded.unknown());

    // Without "before" nothing is compared, and only a head that never reached the fault is unknown.
    let mut alone = raw_of(&folder, "alone");
    alone.fault = Some(FaultOutcome {
        target: "db:5432".to_owned(),
        mode: "slow 2000ms".to_owned(),
        head: vec![calm(1), calm(2)],
        before: None,
    });
    let fault = self::report(&alone).fault.expect("a fault");
    assert!(fault.categories.before.is_none() && fault.changed.is_empty());
    assert!(!fault.unknown());
    let unready = || {
        faulted(
            &folder,
            1,
            Err((Phase::Ready, Cause::Unhealthy)),
            &[],
            (0, [0; 5]),
            0,
        )
    };
    alone.fault.as_mut().expect("a fault").head = vec![unready(), unready()];
    assert!(self::report(&alone).fault.expect("a fault").unknown());
}

#[test]
fn an_unchanged_file_that_changed_on_the_head_is_in_the_second_pass_and_open_connections_grow() {
    let folder = Folder::new();
    // `at` changes from repeat to repeat on both versions: noise, not a change of the state.
    let measured = |side: &str, at: [u32; 2]| {
        let files = at.map(|at| format!("{{\"at\":{at},\"total\":10}}\n"));
        let files: [&[(&str, &str)]; 2] = [
            &[("totals.jsonl", files[0].as_str())],
            &[("totals.jsonl", files[1].as_str())],
        ];
        with_files(&folder, side, &files)
    };
    let mut raw = raw(
        &folder,
        measured("head", [1, 2]),
        Some(measured("before", [3, 4])),
    );
    probe_rule(&mut raw, "orders", Some("/id"), &["/updated_at"]);
    let one = "{\"id\":1,\"status\":\"paid\",\"updated_at\":1}\n";
    let two = format!("{one}{{\"id\":2,\"status\":\"paid\",\"updated_at\":2}}\n");
    let head = passed(
        pass(
            &folder,
            "head",
            1,
            &[
                ("probe.orders.jsonl", one),
                ("totals.jsonl", "{\"at\":5,\"total\":10}\n"),
                ("sub/state.jsonl", "{\"n\":1}\n"),
            ],
            &[("db:5432", 2), ("api:3000", 0)],
        ),
        pass(
            &folder,
            "head",
            2,
            &[
                ("probe.orders.jsonl", two.as_str()),
                ("totals.jsonl", "{\"at\":6,\"total\":10}\n"),
                ("sub/state.jsonl", "{\"n\":2}\n"),
            ],
            &[("db:5432", 4), ("api:3000", 0)],
        ),
    );
    let before = passed(
        pass(
            &folder,
            "before",
            1,
            &[
                ("probe.orders.jsonl", one),
                ("totals.jsonl", "{\"at\":7,\"total\":10}\n"),
                ("sub/state.jsonl", "{\"n\":1}\n"),
            ],
            &[("db:5432", 2)],
        ),
        pass(
            &folder,
            "before",
            2,
            &[
                ("probe.orders.jsonl", one),
                ("totals.jsonl", "{\"at\":8,\"total\":20}\n"),
                ("sub/state.jsonl", "{\"n\":2}\n"),
            ],
            &[("db:5432", 2)],
        ),
    );
    let unchanged = ["probe.orders.jsonl", "totals.jsonl", "sub/state.jsonl"];
    raw.second_pass = Some(SecondPassOutcome {
        unchanged: unchanged.map(str::to_owned).to_vec(),
        head,
        before: Some(before),
    });
    let context = context(&raw, Previous::None, &[]);
    let (report, dir) = written(&folder, "run", &raw, &context);

    let orders = PassFile {
        file: "probe.orders.jsonl".to_owned(),
        before: Some(PassState::Same),
        head: PassState::Changed,
        diffs: vec![Diff {
            at: At::Key { key: json!(2) },
            path: String::new(),
            before: Value::Null,
            head: json!({"id": 2, "status": "paid", "updated_at": 2}),
        }],
    };
    assert_eq!(pass_file(&report, "probe.orders.jsonl"), orders);
    let totals = pass_file(&report, "totals.jsonl");
    assert_eq!(
        (totals.before, totals.head),
        (Some(PassState::Changed), PassState::Same)
    );
    let state = pass_file(&report, "sub/state.jsonl");
    assert_eq!(
        (state.before, state.head),
        (Some(PassState::Changed), PassState::Changed)
    );
    let open = serde_json::to_value(&report.second_pass.as_ref().expect("a pass").open).unwrap();
    let expected = json!([
        {"target": "api:3000", "side": "head", "first": 0, "second": 0},
        {"target": "db:5432", "side": "head", "first": 2, "second": 4},
        {"target": "db:5432", "side": "before", "first": 2, "second": 2},
    ]);
    assert_eq!(open, expected);
    let grown: Vec<&Flag> = report
        .flags
        .iter()
        .filter(|flag| matches!(flag, Flag::ConnectionsGrow { .. }))
        .collect();
    let grow = Flag::ConnectionsGrow {
        target: "db:5432".to_owned(),
        side: Side::Head,
        first: 2,
        second: 4,
    };
    assert_eq!(grown, [&grow]);

    let line = report.line(&dir);
    let passes = line
        .second_pass
        .as_ref()
        .expect("a second pass in the line");
    let files: Vec<(&str, Option<PassState>, u32)> = passes
        .iter()
        .map(|file| (file.file.as_str(), file.before, file.diffs))
        .collect();
    let shown = [
        ("probe.orders.jsonl", Some(PassState::Same), 1),
        ("sub/state.jsonl", Some(PassState::Changed), 1),
    ];
    assert_eq!(files, shown);
    let pass2 = dir.join("diff").join("pass2");
    assert_eq!(passes[0].path, pass2.join("probe.orders.jsonl.json"));
    assert_eq!(passes[0].sample, orders.diffs);
    // `totals.jsonl` held on the head.
    let quiet = report.measures.len() + report.behaviour.as_ref().map_or(0, Vec::len) + 1;
    assert_eq!(
        line.quiet as usize,
        quiet - line.measures.len() - line_files(&line).len()
    );
    let json = serde_json::to_value(&line).unwrap();
    assert_eq!(
        json["secondPass"][1]["path"],
        format!("{}/sub/state.jsonl.json", slashed(&pass2))
    );
    let read = |file: &str| -> Value {
        let text = fs::read_to_string(pass2.join(file)).expect("a diff of the second pass");
        serde_json::from_str(&text).expect("JSON")
    };
    assert_eq!(
        read("sub/state.jsonl.json"),
        json!([{"record": 1, "path": "/n", "before": 1, "head": 2}])
    );
    assert_eq!(read("totals.jsonl.json"), json!([]));
    let loaded = load(&dir).expect("the report loads");
    assert_eq!(loaded.second_pass, report.second_pass);
    let summary = report.summary().to_string();
    let text = "secondPass probe.orders.jsonl before same diffs 1";
    assert!(summary.lines().any(|line| line == text), "{summary}");
}

#[test]
fn a_failed_second_pass_is_a_flag_and_a_failed_head_has_no_second_pass() {
    let folder = Folder::new();
    let before = Some(repeats(&folder, "before", &[100]));
    let mut raw = raw(&folder, repeats(&folder, "head", &[100]), before);
    let file = [("state.jsonl", "{\"n\":1}\n")];
    let failed = |phase| PassPair::Failed(failure_of(phase));
    raw.second_pass = Some(SecondPassOutcome {
        unchanged: vec!["state.jsonl".to_owned()],
        head: passed(
            pass(&folder, "head", 1, &file, &[]),
            pass(&folder, "head", 2, &file, &[]),
        ),
        before: Some(failed(Phase::Ready)),
    });
    let report = report(&raw);

    let state = pass_file(&report, "state.jsonl");
    assert_eq!((state.before, state.head), (None, PassState::Same));
    let flag = Flag::SecondPassFailed {
        side: Side::Before,
        stage: Phase::Ready,
    };
    assert!(report.flags.contains(&flag), "{:?}", report.flags);
    let json = serde_json::to_value(&flag).unwrap();
    assert_eq!(
        json,
        json!({"kind": "second-pass-failed", "side": "before", "stage": "ready"})
    );
    let line = report.line(Utf8Path::new("run"));
    assert_eq!(line.second_pass.as_ref().map(Vec::len), Some(0));

    let second = raw.second_pass.as_mut().expect("a second pass");
    second.head = failed(Phase::Workload);
    let report = self::report(&raw);
    assert!(report.second_pass.is_none());
    let flags = report.flags.iter();
    let failed: Vec<&Flag> = flags
        .filter(|flag| matches!(flag, Flag::SecondPassFailed { .. }))
        .collect();
    let head = Flag::SecondPassFailed {
        side: Side::Head,
        stage: Phase::Workload,
    };
    assert_eq!(failed, [&head, &flag]);
    assert!(report.line(Utf8Path::new("run")).second_pass.is_none());
}

#[test]
fn a_second_pass_file_reads_only_through_plain_folders() {
    let folder = Folder::new();
    let mut raw = raw(&folder, repeats(&folder, "head", &[100]), None);
    let first = pass(&folder, "head", 1, &[("sub", "a file, not a folder")], &[]);
    let second = pass(&folder, "head", 2, &[], &[]);
    raw.second_pass = Some(SecondPassOutcome {
        unchanged: vec!["sub/state.jsonl".to_owned()],
        head: passed(first, second),
        before: None,
    });
    let refused = read_outputs(&raw, &[]);
    assert!(
        matches!(
            refused,
            Err(ReportError::BadResult(BadResult::NotPlainFolder(_)))
        ),
        "{refused:?}"
    );

    // A pass that wrote no such file has no records of it.
    let second = raw.second_pass.as_mut().expect("a second pass");
    second.unchanged = vec!["missing.jsonl".to_owned()];
    let report = self::report(&raw);
    let missing = pass_file(&report, "missing.jsonl");
    assert_eq!((missing.before, missing.head), (None, PassState::Same));
}

#[test]
fn a_pooled_rerun_takes_coverage_fault_and_second_pass_from_the_last_run_with_from() {
    let folder = Folder::new();
    // The first run makes all three: the fixture's run, with its fault and second pass.
    let mut first = fixture_raw(&folder, &[1000, 1000, 1000], Some(&[1000, 1000, 1000]));
    fixture_modes(&folder, &mut first);
    let mut made = context(&first, Previous::None, &[]);
    made.run = "20261022-090000".to_owned();
    let (made, _) = written(&folder, "made", &first, &made);
    for from in [
        made.coverage.as_ref().map(|coverage| &coverage.from),
        made.fault.as_ref().map(|fault| &fault.from),
        made.second_pass.as_ref().map(|pass| &pass.from),
    ] {
        assert_eq!(from, Some(&None), "a result of its own has no `from`");
    }

    // A rerun of the same trees ran none of them: `run` left them out of `Raw`.
    let mut rerun = fixture_raw(&folder, &[1000, 1000, 1000], Some(&[1000, 1000, 1000]));
    rerun.coverage = None;
    let all = BTreeSet::from([Extra::Coverage, Extra::Fault, Extra::SecondPass]);
    let pool = [made.clone()];
    let mut taken = context(&rerun, Previous::None, &pool);
    taken.run = "20261022-091000".to_owned();
    taken.reused = all.clone();
    let (copy, dir) = written(&folder, "copy", &rerun, &taken);

    let from = Some("20261022-090000".to_owned());
    let coverage = copy.coverage.as_ref().expect("the coverage of the pool");
    assert_eq!(coverage.from, from);
    assert_eq!(
        serde_json::to_value(&coverage.called).unwrap(),
        serde_json::to_value(&made.coverage.as_ref().unwrap().called).unwrap()
    );
    let fault = copy.fault.as_ref().expect("the fault of the pool");
    assert_eq!(fault.from, from);
    assert_eq!(fault.changed, made.fault.as_ref().unwrap().changed);
    let pass = copy
        .second_pass
        .as_ref()
        .expect("the second pass of the pool");
    assert_eq!(pass.from, from);
    assert_eq!(pass.files, made.second_pass.as_ref().unwrap().files);
    // The copied open connections still grow, and `write` put the copied diffs in this run.
    assert!(
        copy.flags
            .iter()
            .any(|flag| matches!(flag, Flag::ConnectionsGrow { .. }))
    );
    let pass2 = dir
        .join("diff")
        .join("pass2")
        .join("probe.orders.jsonl.json");
    let written: Value =
        serde_json::from_str(&fs::read_to_string(&pass2).expect("a copied diff")).unwrap();
    assert_eq!(written, serde_json::to_value(&pass.files[0].diffs).unwrap());
    let line = copy.line(&dir);
    assert_eq!(line.fault.as_ref().expect("a fault").from, from);
    let passes = line.second_pass.as_ref().expect("a second pass");
    assert!(!passes.is_empty());
    for file in passes {
        assert_eq!(file.from, from);
        assert!(file.path.starts_with(&dir), "{}", file.path);
    }
    let summary = copy.summary().to_string();
    let text = "fault db:5432 down from 20261022-090000";
    assert!(summary.lines().any(|line| line == text), "{summary}");

    // A copy of the copy keeps where the result came from.
    let pool = [made, copy];
    let mut again = context(&rerun, Previous::None, &pool);
    again.run = "20261022-092000".to_owned();
    again.reused = all;
    let twice = report_of(&rerun, &again, &[]);
    assert_eq!(twice.coverage.expect("a coverage").from, from);
    assert_eq!(twice.fault.expect("a fault").from, from);
    assert_eq!(twice.second_pass.expect("a second pass").from, from);

    // The last run of the pool gives the result, not an older one.
    let [made, copy] = pool;
    let mut fresh = made;
    fresh.run = "20261022-093000".to_owned();
    let pool = [copy, fresh];
    let mut latest = context(&rerun, Previous::None, &pool);
    latest.reused = BTreeSet::from([Extra::Fault]);
    let fault = report_of(&rerun, &latest, &[]).fault.expect("a fault");
    assert_eq!(fault.from.as_deref(), Some("20261022-093000"));

    // Without `reused` the run's own results count, and it has none here.
    let own = report_of(&rerun, &context(&rerun, Previous::None, &pool), &[]);
    assert!(own.coverage.is_none() && own.fault.is_none() && own.second_pass.is_none());
}
