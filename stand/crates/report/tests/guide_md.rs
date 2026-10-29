//! The topic `line` of `skill/references/guide.md` names every field of the `--json` line and
//! every kind of flag with its facts, and nothing else: the agent reads the line by that text.

use std::collections::{BTreeMap, BTreeSet};

use report::{
    At, Cause, Compared, DeadReason, Diff, Failure, Flag, JournalJson, Line, LineFile, LineMeasure,
    Phase, SealJson, Src, Unit, Verdict, VersusPrevious,
};
use serde_json::{Value, json};

const GUIDE: &str = include_str!("../../../../skill/references/guide.md");

fn topic() -> Vec<&'static str> {
    let mut lines = GUIDE.lines().skip_while(|line| *line != "## line");
    assert!(
        lines.next().is_some(),
        "skill/references/guide.md has no `## line`"
    );
    lines.take_while(|line| !line.starts_with("## ")).collect()
}

/// The names in backticks after `prefix` on its line of the topic, up to the first `.` or `:`
/// outside backticks. Text in parentheses describes the name before it and is skipped. A copy of
/// `listed` in `crates/cli/tests/skill_md.rs` of the linter: the two workspaces share no test code.
fn listed(prefix: &str) -> BTreeSet<&'static str> {
    let rest = topic()
        .into_iter()
        .find_map(|line| line.trim_start().strip_prefix(prefix))
        .unwrap_or_else(|| panic!("no line of the topic `line` starts with {prefix}"));
    let mut names = BTreeSet::new();
    let mut depth = 0_u32;
    // Even pieces lie outside backticks, odd ones inside.
    for (i, piece) in rest.split('`').enumerate() {
        if i % 2 == 1 {
            if depth == 0 {
                names.insert(piece);
            }
            continue;
        }
        for mark in piece.chars() {
            match mark {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                '.' | ':' if depth == 0 => return names,
                _ => {}
            }
        }
    }
    names
}

/// The items under `- A flag has`: each kind with the facts in the parentheses right after it.
fn documented_flags() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    let lines = topic();
    let start = lines
        .iter()
        .position(|line| line.starts_with("- A flag has"))
        .expect("a line `- A flag has`");
    let items = lines[start + 1..]
        .iter()
        .take_while(|line| line.starts_with("  - "));
    let flags = items.map(|line| {
        let rest = line
            .trim_start()
            .strip_prefix("- `")
            .unwrap_or_else(|| panic!("a kind in backticks: {line}"));
        let (kind, rest) = rest.split_once('`').expect("a closing backtick");
        let facts = match rest.strip_prefix(" (") {
            Some(inside) => {
                let inside = inside.split_once(')').map_or(inside, |(inside, _)| inside);
                inside.split('`').skip(1).step_by(2).collect()
            }
            None => BTreeSet::new(),
        };
        (kind, facts)
    });
    flags.collect()
}

fn keys(value: &Value) -> BTreeSet<String> {
    let fields = value
        .as_object()
        .unwrap_or_else(|| panic!("an object: {value}"));
    fields.keys().cloned().collect()
}

fn owned(names: BTreeSet<&str>) -> BTreeSet<String> {
    names.into_iter().map(str::to_owned).collect()
}

/// One flag of each kind, and each form of `coverage-unknown`. The `match` stops the build when a
/// kind is added and not made here.
fn every_flag() -> Vec<Flag> {
    let text = String::new;
    let flags = vec![
        Flag::HighNoise { measure: text() },
        Flag::FirstRepeatOutlier { measure: text() },
        Flag::LowRepeats { samples: 1 },
        Flag::NoProfile { service: text() },
        Flag::Unprofiled,
        Flag::LostProcesses {
            service: text(),
            count: 1,
        },
        Flag::UnmappedTime {
            service: text(),
            share: 0.5,
        },
        Flag::MemoryNearLimit {
            service: text(),
            peak: 1,
            limit: 1,
        },
        Flag::NetworkLimitHit {
            repeat: 1,
            target: text(),
            drops: 1,
        },
        Flag::MissingExpose {
            name: text(),
            attempts: 1,
        },
        Flag::BeforeFailed {
            stage: Phase::Setup,
        },
        Flag::CoverageUnknown {
            stage: Some(Phase::Workload),
            service: None,
        },
        Flag::CoverageUnknown {
            stage: None,
            service: Some(text()),
        },
        Flag::CoverageUnknown {
            stage: None,
            service: None,
        },
        Flag::PreviousUnreadable,
        Flag::RecipeChanged,
        Flag::NoResults { file: None },
        Flag::BadResultLine {
            file: text(),
            line: 1,
        },
        Flag::DeadProbe {
            probe: text(),
            reason: DeadReason::NoRecords,
        },
        Flag::ProbeFailed {
            probe: text(),
            side: text(),
            repeat: None,
            code: None,
        },
        Flag::HeadOnlyNoise {
            file: text(),
            path: text(),
        },
        Flag::KilledAtStop {
            service: text(),
            side: text(),
            repeat: 1,
        },
        Flag::BeforeAfterSession {
            taken_at: text(),
            started_at: text(),
        },
        Flag::MeasuredBeforeSeal { run: text() },
        Flag::WideTolerance { max_tolerance: 40 },
    ];
    for flag in &flags {
        match flag {
            Flag::HighNoise { .. }
            | Flag::FirstRepeatOutlier { .. }
            | Flag::LowRepeats { .. }
            | Flag::NoProfile { .. }
            | Flag::Unprofiled
            | Flag::LostProcesses { .. }
            | Flag::UnmappedTime { .. }
            | Flag::MemoryNearLimit { .. }
            | Flag::NetworkLimitHit { .. }
            | Flag::MissingExpose { .. }
            | Flag::BeforeFailed { .. }
            | Flag::CoverageUnknown { .. }
            | Flag::PreviousUnreadable
            | Flag::RecipeChanged
            | Flag::NoResults { .. }
            | Flag::BadResultLine { .. }
            | Flag::DeadProbe { .. }
            | Flag::ProbeFailed { .. }
            | Flag::HeadOnlyNoise { .. }
            | Flag::KilledAtStop { .. }
            | Flag::BeforeAfterSession { .. }
            | Flag::MeasuredBeforeSeal { .. }
            | Flag::WideTolerance { .. } => {}
        }
    }
    flags
}

#[test]
fn the_topic_line_names_every_field_of_the_line() {
    let failure = Failure {
        phase: Phase::Workload,
        service: Some("load".to_owned()),
        repeat: Some(1),
        cause: Cause::Timeout,
        logs: Some("runs/1/repeat-1-head".into()),
        probes: Vec::new(),
    };
    let line = serde_json::to_value(Line::failed(failure, None, Vec::new())).unwrap();
    let mut fields = keys(&line);
    // `cli` prints the line with `next` beside its fields.
    fields.insert("next".to_owned());
    assert_eq!(fields, owned(listed("- The line has")));
    assert_eq!(keys(&line["failure"]), owned(listed("- `failure` has")));

    let time = || "2026-10-22T09:40:00Z".to_owned();
    let seal = SealJson {
        sealed_at: time(),
        max_tolerance: 25,
        journal: vec![
            JournalJson::Sealed {
                sealed_at: time(),
                digest: String::new(),
            },
            JournalJson::Resealed {
                resealed_at: time(),
                digest: String::new(),
                reason: String::new(),
            },
            JournalJson::Cleaned { cleaned_at: time() },
        ],
    };
    let seal = serde_json::to_value(seal).unwrap();
    assert_eq!(keys(&seal), owned(listed("- `seal` has")));
    let entries = seal["journal"].as_array().expect("a journal").iter();
    let journal = entries.flat_map(keys).collect::<BTreeSet<_>>();
    assert_eq!(journal, owned(listed("- A journal entry has")));

    let measure = LineMeasure {
        name: "stand.wall".to_owned(),
        unit: Unit::Ms,
        src: Src::Stand,
        median: 2.0,
        before: Some(Compared {
            median: Some(1.0),
            verdict: Verdict::Regressed,
        }),
        previous: VersusPrevious {
            median: None,
            verdict: Verdict::NoBaseline,
        },
    };
    let measure = serde_json::to_value(measure).unwrap();
    assert_eq!(keys(&measure), owned(listed("- A measure has")));
    let against = &keys(&measure["before"]) | &keys(&measure["previous"]);
    let documented = listed("- `before` and `previous` of a measure have");
    assert_eq!(against, owned(documented));

    let diff = |at: At| Diff {
        at,
        path: "/total".to_owned(),
        before: json!(10),
        head: json!(11),
    };
    let file = LineFile {
        file: "out.jsonl".to_owned(),
        src: Src::Workload,
        noise_known: true,
        diffs: 2,
        sample: vec![
            diff(At::Record { record: 1 }),
            diff(At::Key { key: json!(7) }),
        ],
        path: "runs/1/diff/out.jsonl.json".into(),
    };
    let file = serde_json::to_value(file).unwrap();
    assert_eq!(keys(&file), owned(listed("- A behaviour file has")));
    let diffs = &keys(&file["sample"][0]) | &keys(&file["sample"][1]);
    assert_eq!(diffs, owned(listed("- A diff has")));
}

#[test]
fn the_topic_line_names_every_kind_of_flag_with_its_facts() {
    let mut made = BTreeMap::new();
    for flag in every_flag() {
        let json = serde_json::to_value(&flag).unwrap();
        let kind = json["kind"].as_str().expect("a kind").to_owned();
        let mut facts = keys(&json);
        facts.remove("kind");
        if let Some(other) = made.insert(kind.clone(), facts.clone()) {
            assert_eq!(other, facts, "two forms of {kind} with other facts");
        }
    }
    let documented: BTreeMap<String, BTreeSet<String>> = documented_flags()
        .into_iter()
        .map(|(kind, facts)| (kind.to_owned(), owned(facts)))
        .collect();
    assert_eq!(made, documented);
}
