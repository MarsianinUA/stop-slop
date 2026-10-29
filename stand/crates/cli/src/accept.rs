//! `accept <measure> --allow <bound> --reason "<text>"` (plan, row 67): a shift accepted after the
//! fact, tied to the head tree of the last passed run. It only appends; the line and the report
//! show it in place of the declared cost, `postHoc`, and `stale` once the head moves.

use std::time::SystemTime;

use spec::Change;
use store::Acceptance;

use crate::error::StandError;
use crate::{Env, Exit, Loaded, Reason, context};

pub(crate) fn run(
    measure: &str,
    allow: &str,
    reason: &Reason,
    env: &mut Env<'_>,
) -> Result<Exit, StandError> {
    let change = Change::parse(allow)?;
    if change == Change::Unknown {
        return Err(StandError::UnknownBound);
    }
    // Kept and printed as `Next::Accept` writes it: `+015%` is `+15%`.
    let bound = context::bound(change);
    let loaded = Loaded::read(env)?;
    if !change.within(&loaded.policy) {
        return Err(StandError::TooWide {
            allow: bound.to_string(),
            max: loaded.policy.max_tolerance.get(),
        });
    }
    let now = SystemTime::now();
    let (_lock, session) = loaded.session(env, now)?;
    let history = session.history()?;
    let last = history.iter().rev().find(|record| record.passed);
    let last = last.ok_or(StandError::NoRun)?;
    // `history` gives no folders; the pool of the run's key holds its folder.
    let run = session
        .pool(&last.key)?
        .into_iter()
        .rfind(|run| run.id == last.id);
    let run = run.ok_or(StandError::NoRun)?;
    let report = report::load(&run.dir)?;
    if !report.measures.iter().any(|known| known.name == measure) {
        return Err(StandError::NotInReport {
            measure: measure.to_owned(),
            run: run.id,
        });
    }
    let head_tree = report.head.tree;
    session.accept(Acceptance {
        measure: measure.to_owned(),
        allow: bound.to_string(),
        reason: reason.as_str().to_owned(),
        head_tree: head_tree.clone(),
        accepted_at: now,
    })?;
    let said = writeln!(env.out, "accepted {measure} {bound} for head {head_tree}");
    said.map_err(|source| StandError::Io {
        path: "stdout".into(),
        source,
    })?;
    Ok(Exit::Passed)
}
