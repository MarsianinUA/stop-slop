//! The grades against the declared cost and the acceptances.

use camino::Utf8Path;
use report::{
    BeforeOutcome, Bound, Context, ExpectJson, Flag, Grade, Line, Need, Next, Phase, Previous,
    Report, Topic, Verdict,
};
use serde_json::json;

use super::{
    Folder, HEAD_TREE, accepted, context, declared, failure_of, measure, profiled, raw, raw_of,
    repeats, report_of, sealed, setup_side, verdicts,
};

/// Three repeats of each version; `stand.wall` of "before" is 1000 ms, without noise, so the
/// threshold of the verdict is the larger of 5% and the head's spread.
fn compared_wall(head: &[u64], context: impl FnOnce(&mut Context<'_>)) -> Report {
    let folder = Folder::new();
    let before = Some(repeats(&folder, "before", &[1000, 1000, 1000]));
    let raw = raw(&folder, repeats(&folder, "head", head), before);
    let mut compared = self::context(&raw, Previous::None, &[]);
    compared.seal = Some(sealed("2026-10-22T09:40:00Z", 25));
    context(&mut compared);
    report_of(&raw, &compared, &[])
}

fn expect<'r>(report: &'r Report, name: &str) -> &'r ExpectJson {
    let expect = measure(report, name).expect.as_ref();
    expect.unwrap_or_else(|| panic!("{name} has no expect"))
}

/// The grade of `stand.wall` under `bound`, and that a fixed `api.cpu` is `met` under `none`.
fn wall_grade(head: &[u64], bound: Bound) -> Option<Grade> {
    let report = compared_wall(head, |context| {
        context.declared = vec![declared("stand.wall", bound)];
    });
    assert_eq!(expect(&report, "api.cpu").grade, Some(Grade::Met));
    expect(&report, "stand.wall").grade
}

#[test]
fn none_is_met_within_a_noise_of_5_percent_and_any_shift_is_a_surprise() {
    // +2% within a threshold of 5%; no shift under a spread of 7%; +10% that shows.
    assert_eq!(
        wall_grade(&[1020, 1020, 1020], Bound::None),
        Some(Grade::Met)
    );
    let noisy = [970, 1000, 1040];
    assert_eq!(wall_grade(&noisy, Bound::None), Some(Grade::Unmeasurable));
    assert_eq!(
        wall_grade(&[1100, 1100, 1100], Bound::None),
        Some(Grade::Surprise)
    );
    assert_eq!(
        wall_grade(&[900, 900, 900], Bound::None),
        Some(Grade::Surprise)
    );
}

#[test]
fn a_rise_up_to_x_is_met_beyond_it_exceeded_and_a_drop_a_surprise() {
    let up = Bound::UpTo(10);
    assert_eq!(wall_grade(&[1020, 1020, 1020], up), Some(Grade::Met));
    // Within a noise of 7%, which +10% can tell apart.
    assert_eq!(wall_grade(&[970, 1000, 1040], up), Some(Grade::Met));
    assert_eq!(wall_grade(&[1080, 1080, 1080], up), Some(Grade::Met));
    assert_eq!(wall_grade(&[1150, 1150, 1150], up), Some(Grade::Exceeded));
    // +15% under a threshold of 12% still shows, and beyond +10%.
    assert_eq!(wall_grade(&[1080, 1150, 1218], up), Some(Grade::Exceeded));
    assert_eq!(wall_grade(&[900, 900, 900], up), Some(Grade::Surprise));
    // No shift, but a threshold of 12% is wider than +10%.
    assert_eq!(
        wall_grade(&[940, 1000, 1060], up),
        Some(Grade::Unmeasurable)
    );
}

#[test]
fn a_drop_down_to_x_is_met_beyond_it_exceeded_and_a_rise_a_surprise() {
    let down = Bound::DownTo(10);
    assert_eq!(wall_grade(&[980, 980, 980], down), Some(Grade::Met));
    assert_eq!(wall_grade(&[970, 1000, 1040], down), Some(Grade::Met));
    assert_eq!(wall_grade(&[920, 920, 920], down), Some(Grade::Met));
    assert_eq!(wall_grade(&[850, 850, 850], down), Some(Grade::Exceeded));
    assert_eq!(wall_grade(&[782, 850, 884], down), Some(Grade::Exceeded));
    assert_eq!(wall_grade(&[1100, 1100, 1100], down), Some(Grade::Surprise));
    assert_eq!(
        wall_grade(&[940, 1000, 1060], down),
        Some(Grade::Unmeasurable)
    );
}

#[test]
fn down_is_met_by_a_drop_that_shows_on_a_neutral_measure_too() {
    assert_eq!(wall_grade(&[900, 900, 900], Bound::Down), Some(Grade::Met));
    assert_eq!(
        wall_grade(&[1100, 1100, 1100], Bound::Down),
        Some(Grade::Surprise)
    );
    // No drop shows: a surprise, unless the noise could hide one.
    assert_eq!(
        wall_grade(&[1000, 1000, 1000], Bound::Down),
        Some(Grade::Surprise)
    );
    let noisy = [940, 1000, 1060];
    assert_eq!(wall_grade(&noisy, Bound::Down), Some(Grade::Unmeasurable));

    // 8 calls to `api:3000` instead of 10: a neutral measure only changes, and `down` grades the
    // sign of the shift.
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[1000, 1000, 1000]);
    for repeat in &mut head {
        repeat.instances[1].metrics.ports[0].connections = 8;
    }
    let raw = raw(
        &folder,
        head,
        Some(repeats(&folder, "before", &[1000, 1000, 1000])),
    );
    let mut compared = context(&raw, Previous::None, &[]);
    compared.declared = vec![declared("api:3000.connections", Bound::Down)];
    let report = report_of(&raw, &compared, &[]);
    let connections = measure(&report, "api:3000.connections");
    assert_eq!(verdicts(connections).0, Some(Verdict::Changed));
    assert_eq!(
        expect(&report, "api:3000.connections").grade,
        Some(Grade::Met)
    );
}

#[test]
fn a_rise_from_zero_exceeds_any_bound_and_has_no_template() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[1000, 1000, 1000]);
    for (index, repeat) in head.iter_mut().enumerate() {
        let (log, cut) = folder.log(
            &format!("head-api-{index}"),
            "ERROR: boom
",
            "",
        );
        repeat.instances[0].log = log;
        repeat.instances[0].log_cut = cut;
    }
    let raw = raw(
        &folder,
        head,
        Some(repeats(&folder, "before", &[1000, 1000, 1000])),
    );
    let mut compared = context(&raw, Previous::None, &[]);
    compared.seal = Some(sealed("2026-10-22T09:40:00Z", 1000));
    compared.declared = vec![declared("api.error-lines", Bound::UpTo(100))];
    let report = report_of(&raw, &compared, &[]);

    let errors = expect(&report, "api.error-lines");
    assert_eq!(errors.grade, Some(Grade::Exceeded));
    let next = report.line(Utf8Path::new("run")).next;
    assert!(next.is_empty(), "{next:?}");
}

#[test]
fn the_own_hot_time_of_new_code_is_a_quiet_unmeasurable_and_a_rise_from_more_grades() {
    let folder = Folder::new();
    let util = ("src/util.ts", "format", 3, 5, 50.0);
    let head = [("src/orders.ts", "build", 10, 21, 300.0), util];
    let graded = |before: &[(&str, &str, u32, u32, f64)], changed: &str, entry: Option<Bound>| {
        let raw = raw(
            &folder,
            profiled(&folder, "head", &head),
            Some(profiled(&folder, "before", before)),
        );
        let mut compared = context(&raw, Previous::None, &[]);
        compared.seal = Some(sealed("2026-10-22T09:40:00Z", 1000));
        compared.changed = Some([changed.to_owned()].into());
        compared.declared = entry
            .map(|bound| declared("stand.hot-owned", bound))
            .into_iter()
            .collect();
        report_of(&raw, &compared, &[])
    };
    let new = graded(&[util], "src/orders.ts", None);
    let owned = expect(&new, "stand.hot-owned");
    assert_eq!(
        (owned.change.as_str(), owned.reason.as_deref(), owned.grade),
        ("none", Some("new-code"), Some(Grade::Unmeasurable))
    );
    assert_eq!(expect(&new, "stand.hot-collateral").grade, Some(Grade::Met));
    let line = new.line(Utf8Path::new("run"));
    assert!(names_of(&line).is_empty(), "{:?}", names_of(&line));
    assert_eq!(line.unmeasurable, 1);
    assert!(line.next.is_empty(), "{:?}", line.next);
    // An entry changes the bound, not the grade; `unknown` still asks a person.
    let bounded = graded(&[util], "src/orders.ts", Some(Bound::UpTo(10)));
    let owned = expect(&bounded, "stand.hot-owned");
    assert_eq!(
        (owned.change.as_str(), owned.reason.as_deref(), owned.grade),
        ("+10%", Some("new-code"), Some(Grade::Unmeasurable))
    );
    assert!(names_of(&bounded.line(Utf8Path::new("run"))).is_empty());
    let asked = graded(&[util], "src/orders.ts", Some(Bound::Unknown));
    assert_eq!(expect(&asked, "stand.hot-owned").grade, None);
    assert_eq!(asked.needs.len(), 1, "{:?}", asked.needs);
    // From 100 ms the rise shows under `none`; at 0 on both sides nothing moved.
    let grown = graded(
        &[("src/orders.ts", "build", 10, 21, 100.0), util],
        "src/orders.ts",
        None,
    );
    let owned = expect(&grown, "stand.hot-owned");
    assert_eq!(
        (owned.reason.as_deref(), owned.grade),
        (None, Some(Grade::Surprise))
    );
    let line = grown.line(Utf8Path::new("run"));
    assert_eq!(names_of(&line), ["stand.hot-owned"]);
    let untouched = graded(&[util], "src/other.ts", None);
    let owned = expect(&untouched, "stand.hot-owned");
    assert_eq!(
        (owned.reason.as_deref(), owned.grade),
        (None, Some(Grade::Met))
    );
}

#[test]
fn unknown_has_no_grade_and_asks_a_person_in_needs() {
    let report = compared_wall(&[1300, 1300, 1300], |context| {
        context.declared = vec![declared("stand.wall", Bound::Unknown)];
    });

    let wall = expect(&report, "stand.wall");
    assert_eq!((wall.change.as_str(), wall.grade), ("unknown", None));
    let need = Need {
        measure: "stand.wall".to_owned(),
        reason: "the cost of stand.wall".to_owned(),
    };
    assert_eq!(report.needs, [need]);
    let line = report.line(Utf8Path::new("run"));
    assert_eq!(line.needs, report.needs);
    assert_eq!(names_of(&line), ["stand.wall"]);
    assert!(line.next.is_empty(), "{:?}", line.next);
    let json = serde_json::to_value(&line).unwrap();
    assert!(json["measures"][0]["expect"]["grade"].is_null());
    let summary = report.summary().to_string();
    let asked = "needs stand.wall: the cost of stand.wall";
    assert!(summary.lines().any(|line| line == asked), "{summary}");
}

fn names_of(line: &Line) -> Vec<&str> {
    let names = line.measures.iter();
    names.map(|measure| measure.name.as_str()).collect()
}

#[test]
fn the_accept_template_rounds_the_shift_up_to_5_percent_within_max_tolerance() {
    let accept = |bound| Next::Accept {
        measure: "stand.wall".to_owned(),
        bound,
    };
    let next = |head: &[u64], bound: Option<Bound>| {
        let report = compared_wall(head, |context| {
            context.declared = bound
                .map(|bound| declared("stand.wall", bound))
                .into_iter()
                .collect();
        });
        report.line(Utf8Path::new("run")).next
    };
    // `exceeded` at +13.2%, a `surprise` of a 7% drop, `down` within the noise.
    let exceeded = next(&[1132, 1132, 1132], Some(Bound::UpTo(10)));
    assert_eq!(exceeded, [accept(Bound::UpTo(15))]);
    assert_eq!(next(&[930, 930, 930], None), [accept(Bound::DownTo(10))]);
    let held = next(&[1000, 1000, 1000], Some(Bound::Down));
    assert_eq!(held, [accept(Bound::None)]);
    // An exact +15% stays 15, and +40% is wider than `maxTolerance` 25.
    assert_eq!(next(&[1150, 1150, 1150], None), [accept(Bound::UpTo(15))]);
    assert!(next(&[1400, 1400, 1400], None).is_empty());
    // +55% is 11.000000000000002 times 5% as a float, and still 55.
    let wide = compared_wall(&[1550, 1550, 1550], |context| {
        context.seal = Some(sealed("2026-10-22T09:40:00Z", 100));
    });
    let rounded = wide.line(Utf8Path::new("run")).next;
    assert_eq!(rounded, [accept(Bound::UpTo(55))]);
    // An `unmeasurable` entry asks for a wider bound or less noise.
    let noisy = next(&[940, 1000, 1060], Some(Bound::UpTo(10)));
    assert_eq!(noisy, [Next::Guide(Some(Topic::Expect))]);
    // Without a seal there is no `maxTolerance` to keep to.
    let report = compared_wall(&[930, 930, 930], |context| context.seal = None);
    assert!(report.line(Utf8Path::new("run")).next.is_empty());
}

#[test]
fn an_unmeasurable_measure_without_an_entry_is_quiet_and_counted() {
    let quiet = compared_wall(&[970, 1000, 1040], |_| {});
    let line = quiet.line(Utf8Path::new("run"));
    assert_eq!(
        expect(&quiet, "stand.wall").grade,
        Some(Grade::Unmeasurable)
    );
    assert_eq!(expect(&quiet, "stand.wall").reason, None);
    assert!(names_of(&line).is_empty(), "{:?}", names_of(&line));
    assert_eq!(line.unmeasurable, 1);
    assert_eq!(line.quiet as usize, quiet.measures.len());

    let declared = compared_wall(&[970, 1000, 1040], |context| {
        context.declared = vec![declared("stand.wall", Bound::None)];
    });
    let line = declared.line(Utf8Path::new("run"));
    assert_eq!(names_of(&line), ["stand.wall"]);
    assert_eq!(line.unmeasurable, 0);
    let json = serde_json::to_value(&line).unwrap();
    assert_eq!(json["unmeasurable"], 0);
}

#[test]
fn the_last_acceptance_of_this_head_replaces_the_bound_and_one_of_another_head_is_stale() {
    let report = compared_wall(&[1200, 1200, 1200], |context| {
        context.declared = vec![declared("stand.wall", Bound::UpTo(10))];
        context.accepted = vec![
            accepted(
                "stand.wall",
                Bound::UpTo(15),
                "other",
                "2026-10-22T09:50:00Z",
            ),
            accepted(
                "stand.wall",
                Bound::UpTo(25),
                HEAD_TREE,
                "2026-10-22T09:51:00Z",
            ),
            accepted(
                "stand.wall",
                Bound::UpTo(20),
                HEAD_TREE,
                "2026-10-22T09:52:00Z",
            ),
            // Another head only: `api.cpu` is `met` and still in the line.
            accepted("api.cpu", Bound::UpTo(10), "other", "2026-10-22T09:53:00Z"),
        ];
    });

    let wall = expect(&report, "stand.wall");
    let held = ExpectJson {
        change: "+20%".to_owned(),
        reason: Some("accepted +20%".to_owned()),
        grade: Some(Grade::Met),
        post_hoc: true,
        stale: false,
        acceptances: 3,
    };
    assert_eq!(wall, &held);
    let cpu = expect(&report, "api.cpu");
    assert_eq!(
        (cpu.grade, cpu.stale, cpu.post_hoc),
        (Some(Grade::Met), true, false)
    );
    assert_eq!((cpu.change.as_str(), cpu.reason.as_deref()), ("none", None));
    let line = report.line(Utf8Path::new("run"));
    assert_eq!(names_of(&line), ["stand.wall", "api.cpu"]);
    assert!(line.next.is_empty(), "{:?}", line.next);
    let json = serde_json::to_value(&line).unwrap();
    assert_eq!(json["measures"][0]["expect"]["postHoc"], true);
    assert_eq!(json["measures"][1]["expect"]["stale"], true);

    // Only the acceptance of another head: the declared bound holds, and it is exceeded.
    let report = compared_wall(&[1200, 1200, 1200], |context| {
        context.declared = vec![declared("stand.wall", Bound::UpTo(10))];
        context.accepted = vec![accepted(
            "stand.wall",
            Bound::UpTo(25),
            "other",
            "2026-10-22T09:50:00Z",
        )];
    });
    let wall = expect(&report, "stand.wall");
    assert_eq!(
        (wall.change.as_str(), wall.grade),
        ("+10%", Some(Grade::Exceeded))
    );
    assert_eq!(
        (wall.post_hoc, wall.stale, wall.acceptances),
        (false, true, 1)
    );
    let next = report.line(Utf8Path::new("run")).next;
    let template = Next::Accept {
        measure: "stand.wall".to_owned(),
        bound: Bound::UpTo(20),
    };
    assert_eq!(next, [template]);
}

#[test]
fn an_entry_of_a_measure_the_report_lacks_is_unknown_expect() {
    let typo = |context: &mut Context<'_>| {
        context.declared = vec![
            declared("stand.wall", Bound::UpTo(10)),
            declared("api.gpu", Bound::UpTo(10)),
        ];
    };
    let report = compared_wall(&[1000, 1000, 1000], typo);
    let unknown: Vec<&Flag> = report
        .flags
        .iter()
        .filter(|flag| matches!(flag, Flag::UnknownExpect { .. }))
        .collect();
    let flag = Flag::UnknownExpect {
        measure: "api.gpu".to_owned(),
    };
    assert_eq!(unknown, [&flag]);
    let json = serde_json::to_value(&flag).unwrap();
    assert_eq!(
        json,
        json!({"kind": "unknown-expect", "measure": "api.gpu"})
    );

    // A plain run grades nothing, so its entries are not looked at.
    let folder = Folder::new();
    let plain = raw_of(&folder, "plain");
    let mut explored = context(&plain, Previous::None, &[]);
    typo(&mut explored);
    let report = report_of(&plain, &explored, &[]);
    assert!(
        !report
            .flags
            .iter()
            .any(|flag| matches!(flag, Flag::UnknownExpect { .. }))
    );
    assert!(
        report
            .measures
            .iter()
            .all(|measure| measure.expect.is_none())
    );
    assert!(report.needs.is_empty());
}

#[test]
fn a_failed_before_grades_nothing() {
    let folder = Folder::new();
    let mut raw = raw(&folder, repeats(&folder, "head", &[100, 100, 100]), None);
    raw.setup.before = Some(setup_side(&folder, "before", &[]));
    raw.before = Some(BeforeOutcome::Failed(failure_of(Phase::Setup)));
    let mut compared = context(&raw, Previous::None, &[]);
    compared.declared = vec![declared("stand.wall", Bound::Unknown)];
    let report = report_of(&raw, &compared, &[]);

    assert!(
        report
            .measures
            .iter()
            .all(|measure| measure.expect.is_none())
    );
    assert!(report.needs.is_empty());
    let line = report.line(Utf8Path::new("run"));
    assert!(line.measures.is_empty() && line.next.is_empty());
}
