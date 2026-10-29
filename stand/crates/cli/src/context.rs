//! `report::Context`: what only `cli` knows about a run. `Raw` goes to `report` as it is (plan,
//! row 85).

use std::collections::BTreeSet;
use std::time::SystemTime;

use camino::Utf8PathBuf;
use report::{Bound, Context, Declared, Extra, PastRun, PostHoc, Previous, Report, SetupFrom};
use spec::{Change, Stand};
use store::{Acceptance, RunRecord};

/// Everything the context is made of, gathered by `measure`.
pub(crate) struct Known<'r> {
    pub run: &'r str,
    pub created_at: SystemTime,
    pub root: Utf8PathBuf,
    pub head_tree: String,
    pub stand: &'r Stand,
    pub history: &'r [RunRecord],
    pub acceptances: &'r [Acceptance],
    pub previous: Previous<'r>,
    pub pool: &'r [Report],
    pub setup_from: SetupFrom,
    pub reused: BTreeSet<Extra>,
}

pub(crate) fn of(known: Known<'_>) -> Context<'_> {
    Context {
        run: known.run.to_owned(),
        created_at: known.created_at,
        root: known.root,
        head_tree: known.head_tree,
        changed: None,
        before: None,
        seal: None,
        // The earlier runs only: `new_run` already wrote this one.
        history: known
            .history
            .iter()
            .filter(|record| record.id != known.run)
            .map(|record| PastRun {
                run: record.id.clone(),
                created_at: record.created_at,
                head_tree: record.key.head_tree.clone(),
                compared: record.key.before_tree.is_some(),
                passed: record.passed,
            })
            .collect(),
        declared: known
            .stand
            .expect
            .iter()
            .map(|cost| Declared {
                measure: cost.measure.clone(),
                bound: bound(cost.change),
                reason: cost.reason.clone(),
                post_hoc: None,
            })
            .collect(),
        // `accept` wrote only what `Change::parse` took, so every entry reads back.
        accepted: known
            .acceptances
            .iter()
            .filter_map(|acceptance| {
                let change = Change::parse(&acceptance.allow).ok()?;
                Some(Declared {
                    measure: acceptance.measure.clone(),
                    bound: bound(change),
                    reason: acceptance.reason.clone(),
                    post_hoc: Some(PostHoc {
                        head_tree: acceptance.head_tree.clone(),
                        accepted_at: acceptance.accepted_at,
                    }),
                })
            })
            .collect(),
        previous: known.previous,
        pool: known.pool,
        setup_from: known.setup_from,
        reused: known.reused,
    }
}

fn bound(change: Change) -> Bound {
    match change {
        Change::None => Bound::None,
        Change::UpTo(percent) => Bound::UpTo(percent.get()),
        Change::DownTo(percent) => Bound::DownTo(percent.get()),
        Change::Down => Bound::Down,
        Change::Unknown => Bound::Unknown,
    }
}
