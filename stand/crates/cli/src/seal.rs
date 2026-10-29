//! The seal of the recipe (plan, row 65), under the lock: the first `run --before` of a session
//! copies the folder of the stand file into the session, and every run with `--before` reads that
//! copy. A changed, new or gone file of the live folder is exit 2 until it is put back or
//! `--reseal --reason` seals it again. Runs without `--before` read the live folder.
//!
//! A run writes its seal only after its pull (batch review 6): a first run that the budget or a
//! missing image refuses leaves no seal, so the fix of its recipe needs no reseal.

use std::time::SystemTime;

use camino::Utf8Path;
use source::Before;
use spec::{Policy, Stand};
use store::{Sealed, Session};

use crate::error::StandError;
use crate::{Reason, STAND_FILE, context};

/// The stand a run with `--before` reads, and its seal; writes nothing, and `--dry-run` stops
/// here. `None`: the run seals the live folder, which it reads until then (`seal`).
pub(crate) fn compare(
    session: &Session,
    live: Stand,
    before: &Before,
    reseal: Option<&Reason>,
    policy: &Policy,
) -> Result<(Stand, Option<Sealed>), StandError> {
    if reseal.is_some() {
        return Ok((live, None));
    }
    let Some(sealed) = session.sealed()? else {
        return Ok((live, None));
    };
    let stand = read(&sealed, policy)?;
    let files = live.recipe.changed(&stand.recipe);
    if !files.is_empty() {
        return Err(StandError::RecipeChanged {
            files,
            before: context::intent(before),
        });
    }
    Ok((stand, Some(sealed)))
}

/// The seal of a run that `compare` gave none: the first of the session, or the reseal. The run
/// reads the copy from here on, as every later run will.
pub(crate) fn seal(
    session: &Session,
    live: &Stand,
    reseal: Option<&Reason>,
    policy: &Policy,
    now: SystemTime,
) -> Result<(Stand, Sealed), StandError> {
    let folder = folder(live);
    let digest = &live.recipe.digest;
    let sealed = match (session.sealed()?, reseal) {
        (Some(_), Some(reason)) => session.reseal(folder, digest, now, reason.as_str())?,
        // A reseal with no seal yet is a first seal: there is nothing to start over.
        _ => session.seal(folder, digest, now)?,
    };
    Ok((read(&sealed, policy)?, sealed))
}

/// `spec::load` on the copy: the seal compares with the same code that reads a recipe.
fn read(sealed: &Sealed, policy: &Policy) -> Result<Stand, StandError> {
    Ok(spec::load(&sealed.dir.join(STAND_FILE), policy)?)
}

fn folder(stand: &Stand) -> &Utf8Path {
    stand
        .file
        .parent()
        .expect("a stand file lies in its folder")
}
