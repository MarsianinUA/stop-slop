//! `report::Context`: what only `cli` knows about a run. `Raw` goes to `report` as it is (plan,
//! row 85).

use std::collections::BTreeSet;
use std::time::SystemTime;

use camino::Utf8PathBuf;
use report::{
    BeforeInfo, Bound, Context, Declared, Extra, Intent, Journal, PastRun, PostHoc, Previous,
    Report, SealInfo, SetupFrom,
};
use source::{Before, Resolved};
use spec::{Change, Policy, Stand};
use store::{Acceptance, JournalEntry, RunRecord, Sealed};

/// Everything the context is made of, gathered by `measure`.
pub(crate) struct Known<'r> {
    pub run: &'r str,
    pub created_at: SystemTime,
    pub root: Utf8PathBuf,
    pub head_tree: String,
    /// With `--before`: what it resolved to and the paths the head changed against it.
    pub before: Option<(&'r Resolved, Vec<Utf8PathBuf>)>,
    pub stand: &'r Stand,
    pub seal: Option<(&'r Sealed, Policy)>,
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
        changed: known
            .before
            .as_ref()
            .map(|(_, changed)| changed.iter().map(ToString::to_string).collect()),
        before: known.before.as_ref().map(|(resolved, _)| before(resolved)),
        seal: known.seal.map(|(sealed, policy)| SealInfo {
            sealed_at: sealed.sealed_at,
            max_tolerance: policy.max_tolerance.get(),
            journal: sealed.journal.iter().map(journal).collect(),
        }),
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

fn before(resolved: &Resolved) -> BeforeInfo {
    BeforeInfo {
        intent: match &resolved.intent {
            Before::Session => Intent::Session,
            Before::Ref(name) => Intent::Ref(name.clone()),
        },
        treeish: resolved.treeish.to_string(),
        tree: resolved.tree.to_string(),
        taken_at: resolved.taken_at,
    }
}

fn journal(entry: &JournalEntry) -> Journal {
    match entry.clone() {
        JournalEntry::Sealed { sealed_at, digest } => Journal::Sealed { sealed_at, digest },
        JournalEntry::Resealed {
            resealed_at,
            digest,
            reason,
        } => Journal::Resealed {
            resealed_at,
            digest,
            reason,
        },
        JournalEntry::Cleaned { cleaned_at } => Journal::Cleaned { cleaned_at },
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
