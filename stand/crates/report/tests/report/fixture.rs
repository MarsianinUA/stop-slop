//! `make` on the raw outputs and context of `shared/fixtures/report.json`.

use report::{Bound, Declared, Previous};
use serde_json::Value;

use super::{
    FIXTURE, Folder, HEAD_TREE, PREVIOUS, accepted, at, context, declared, fixture_modes,
    fixture_raw, past, report_of, sealed,
};

#[test]
fn make_on_the_raw_outputs_and_context_of_the_fixture_gives_the_fixture() {
    let folder = Folder::new();
    let earlier = fixture_raw(&folder, &[41900, 41800, 42000], None);
    let mut earlier_context = context(&earlier, Previous::None, &[]);
    earlier_context.run = PREVIOUS.to_owned();
    earlier_context.created_at = at("2026-10-22T09:45:00Z");
    let previous = report_of(&earlier, &earlier_context, &[]);
    let mut raw = fixture_raw(
        &folder,
        &[42210, 41877, 42502],
        Some(&[41010, 40870, 41502]),
    );
    fixture_modes(&folder, &mut raw);
    let mut context = context(&raw, Previous::Read(&previous), &[]);
    context.setup_from.before = Some(PREVIOUS.to_owned());
    context.changed = Some(["src/orders/orders.service.ts".to_owned()].into());
    // An early plain run of another head, then the previous run, the first comparison.
    context.seal = Some(sealed("2026-10-22T09:40:00Z", 25));
    context.history = vec![
        past(
            "20261022-091500",
            "2026-10-22T09:15:00Z",
            "other",
            false,
            true,
        ),
        past(PREVIOUS, "2026-10-22T09:45:00Z", "other", true, true),
    ];
    let reason = |entry: Declared, reason: &str| Declared {
        reason: reason.to_owned(),
        ..entry
    };
    context.declared = vec![
        reason(
            declared("stand.wall", Bound::UpTo(10)),
            "one more index lookup per order",
        ),
        reason(declared("db.cpu", Bound::Unknown), "a new query plan"),
    ];
    let cpu = accepted(
        "api.cpu",
        Bound::UpTo(20),
        HEAD_TREE,
        "2026-10-22T09:50:00Z",
    );
    context.accepted = vec![reason(cpu, "the report formats every order")];
    let report = report_of(&raw, &context, &[]);

    let expected: Value = serde_json::from_str(FIXTURE).expect("the fixture is JSON");
    let made = serde_json::to_value(&report).expect("the report serializes");
    assert_eq!(
        made,
        expected,
        "made:\n{}",
        serde_json::to_string_pretty(&made).unwrap()
    );
}
