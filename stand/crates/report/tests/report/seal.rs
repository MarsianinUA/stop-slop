//! The seal, the attempts and the comparisons.

use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use report::{Flag, Journal, Line, Phase, Previous, Report, SealInfo, load};
use serde_json::json;

use super::{
    DIGEST, Folder, HEAD_TREE, at, context, failure_of, past, raw, raw_of, repeats, report,
    report_of, sealed, written,
};

fn seal_flags(report: &Report) -> Vec<&Flag> {
    let flags = report.flags.iter().filter(|flag| {
        matches!(
            flag,
            Flag::BeforeAfterSession { .. }
                | Flag::MeasuredBeforeSeal { .. }
                | Flag::WideTolerance { .. }
        )
    });
    flags.collect()
}

#[test]
fn the_seal_its_journal_and_the_comparisons_since_it_are_in_the_report_and_the_line() {
    let folder = Folder::new();
    let before = Some(repeats(&folder, "before", &[100]));
    let raw = raw(&folder, repeats(&folder, "head", &[100]), before);
    let mut compared = context(&raw, Previous::None, &[]);
    let old = "0".repeat(64);
    compared.seal = Some(SealInfo {
        journal: vec![
            Journal::Sealed {
                sealed_at: at("2026-10-22T08:00:00Z"),
                digest: old.clone(),
            },
            Journal::Cleaned {
                cleaned_at: at("2026-10-22T08:30:00Z"),
            },
            Journal::Resealed {
                resealed_at: at("2026-10-22T09:40:00Z"),
                digest: DIGEST.to_owned(),
                reason: "a new probe".to_owned(),
            },
        ],
        ..sealed("2026-10-22T09:40:00Z", 25)
    });
    // Only the passed runs with `--before` since the reseal count: not one before it, a failed
    // one or a plain one.
    compared.history = vec![
        past(
            "20261022-093000",
            "2026-10-22T09:30:00Z",
            "other",
            true,
            true,
        ),
        past(
            "20261022-094100",
            "2026-10-22T09:41:00Z",
            "other",
            true,
            false,
        ),
        past(
            "20261022-094200",
            "2026-10-22T09:42:00Z",
            "other",
            false,
            true,
        ),
        past(
            "20261022-094500",
            "2026-10-22T09:45:00Z",
            "other",
            true,
            true,
        ),
    ];
    let report = report_of(&raw, &compared, &[]);

    let json = serde_json::to_value(&report).unwrap();
    let seal = json!({
        "sealedAt": "2026-10-22T09:40:00Z",
        "maxTolerance": 25,
        "journal": [
            {"sealedAt": "2026-10-22T08:00:00Z", "digest": old},
            {"cleanedAt": "2026-10-22T08:30:00Z"},
            {"resealedAt": "2026-10-22T09:40:00Z", "digest": DIGEST, "reason": "a new probe"},
        ],
    });
    assert_eq!(json["seal"], seal);
    assert_eq!(
        (&json["attempts"], &json["comparisons"]),
        (&json!(1), &json!(2))
    );
    let back: Report = serde_json::from_value(json).expect("the report reads back");
    assert_eq!(back.seal, report.seal);
    assert!(seal_flags(&report).is_empty(), "{:?}", report.flags);
    let line = report.line(Utf8Path::new("run"));
    assert_eq!(line.seal, report.seal);
    assert_eq!((line.attempts, line.comparisons), (1, Some(2)));
    let summary = report.summary().to_string();
    let reseal =
        format!("journal digest {DIGEST} reason a new probe resealedAt 2026-10-22T09:40:00Z");
    let printed = [
        "seal 2026-10-22T09:40:00Z  maxTolerance 25",
        "journal cleanedAt 2026-10-22T08:30:00Z",
        reseal.as_str(),
        "attempts 1  comparisons 2",
    ];
    for text in printed {
        assert!(
            summary.lines().any(|line| line == text),
            "{text} not in\n{summary}"
        );
    }

    // Without `--before` neither is there, whatever the context holds.
    let plain = raw_of(&folder, "plain");
    let mut explored = context(&plain, Previous::None, &[]);
    explored.seal = Some(sealed("2026-10-22T09:40:00Z", 40));
    explored.history = compared.history.clone();
    let report = report_of(&plain, &explored, &[]);
    let json = serde_json::to_value(&report).unwrap();
    assert!(
        json["seal"].is_null() && json["comparisons"].is_null(),
        "{json}"
    );
    assert!(seal_flags(&report).is_empty(), "{:?}", report.flags);
    let line = Line::failed(failure_of(Phase::Workload), Some(&plain), Vec::new());
    assert!(line.seal.is_none() && line.attempts == 0 && line.comparisons.is_none());
}

#[test]
fn a_wide_tolerance_a_run_before_the_seal_and_a_late_before_are_flags() {
    let folder = Folder::new();
    let before = Some(repeats(&folder, "before", &[100]));
    let raw = raw(&folder, repeats(&folder, "head", &[100]), before);
    let mut compared = context(&raw, Previous::None, &[]);
    compared.seal = Some(sealed("2026-10-22T09:40:00Z", 40));
    // "Before" is from 09:12 and the session began at 09:10 with a run of this head. The failed
    // run, the run of another head and the one after the seal measured nothing before it.
    compared.history = vec![
        past(
            "20261022-091000",
            "2026-10-22T09:10:00Z",
            HEAD_TREE,
            false,
            true,
        ),
        past(
            "20261022-092000",
            "2026-10-22T09:20:00Z",
            HEAD_TREE,
            false,
            true,
        ),
        past(
            "20261022-093000",
            "2026-10-22T09:30:00Z",
            HEAD_TREE,
            false,
            false,
        ),
        past(
            "20261022-093500",
            "2026-10-22T09:35:00Z",
            "other",
            false,
            true,
        ),
        past(
            "20261022-094500",
            "2026-10-22T09:45:00Z",
            HEAD_TREE,
            true,
            true,
        ),
    ];
    let report = report_of(&raw, &compared, &[]);

    let expected = [
        Flag::BeforeAfterSession {
            taken_at: "2026-10-22T09:12:00Z".to_owned(),
            started_at: "2026-10-22T09:10:00Z".to_owned(),
        },
        Flag::MeasuredBeforeSeal {
            run: "20261022-092000".to_owned(),
        },
        Flag::WideTolerance { max_tolerance: 40 },
    ];
    assert_eq!(seal_flags(&report), expected.iter().collect::<Vec<_>>());
    let late = json!({
        "kind": "before-after-session",
        "takenAt": "2026-10-22T09:12:00Z",
        "startedAt": "2026-10-22T09:10:00Z",
    });
    assert_eq!(serde_json::to_value(&expected[0]).unwrap(), late);
    let wide = json!({"kind": "wide-tolerance", "maxTolerance": 40});
    assert_eq!(serde_json::to_value(&expected[2]).unwrap(), wide);

    // At 25, with "before" taken first and no run of this head before the seal: none.
    compared.seal = Some(sealed("2026-10-22T09:40:00Z", 25));
    compared.history = vec![past(
        "20261022-091500",
        "2026-10-22T09:15:00Z",
        "other",
        false,
        true,
    )];
    assert!(seal_flags(&report_of(&raw, &compared, &[])).is_empty());
    // With no earlier run the seal starts the session.
    compared.seal = Some(sealed("2026-10-22T09:00:00Z", 25));
    compared.history.clear();
    let report = report_of(&raw, &compared, &[]);
    let started = Flag::BeforeAfterSession {
        taken_at: "2026-10-22T09:12:00Z".to_owned(),
        started_at: "2026-10-22T09:00:00Z".to_owned(),
    };
    assert_eq!(seal_flags(&report), [&started]);
}

#[test]
fn attempts_count_the_pooled_runs_and_an_unreadable_report_is_left_out() {
    let folder = Folder::new();
    let dirs: Vec<Utf8PathBuf> = ["a", "b", "c"]
        .iter()
        .map(|name| {
            let raw = raw_of(&folder, name);
            let context = context(&raw, Previous::None, &[]);
            written(&folder, name, &raw, &context).1
        })
        .collect();
    fs::write(dirs[1].join("report.json"), "{").expect("break a pooled report");
    // As `cli` pools: the reports that load.
    let pool: Vec<Report> = dirs.iter().filter_map(|dir| load(dir).ok()).collect();
    let raw = raw_of(&folder, "head");
    let report = report_of(&raw, &context(&raw, Previous::None, &pool), &[]);

    assert_eq!(report.attempts, 3);
    assert_eq!(report.line(Utf8Path::new("run")).attempts, 3);
    assert_eq!(self::report(&raw).attempts, 1);
}
