//! The seal of the recipe (plan, row 65), under the lock: the first `run --before` of a session
//! copies the folder of the stand file into the session, and every run with `--before` reads that
//! copy. A changed, new or gone file of the live folder is exit 2 until it is put back or
//! `--reseal --reason` seals it again. Runs without `--before` read the live folder.

use std::time::SystemTime;

use camino::Utf8Path;
use source::Before;
use spec::{Policy, Stand};
use store::{Sealed, Session};

use crate::error::StandError;
use crate::{Reason, STAND_FILE};

/// The stand of a run with `--before`, read from the sealed copy even right after sealing, and
/// the seal.
pub(crate) fn check(
    session: &Session,
    live: &Stand,
    before: &Before,
    reseal: Option<&Reason>,
    policy: &Policy,
    now: SystemTime,
) -> Result<(Stand, Sealed), StandError> {
    let folder = folder(live);
    let digest = &live.recipe.digest;
    let sealed = match (session.sealed()?, reseal) {
        // A reseal with no seal yet is a first seal: there is nothing to start over.
        (None, _) => session.seal(folder, digest, now)?,
        (Some(_), Some(reason)) => session.reseal(folder, digest, now, reason.as_str())?,
        (Some(sealed), None) => {
            let stand = read(&sealed, policy)?;
            refuse_changed(live, &stand, before)?;
            return Ok((stand, sealed));
        }
    };
    Ok((read(&sealed, policy)?, sealed))
}

/// `--dry-run` compares with the seal and never seals: the stand to check and the seal, if any.
pub(crate) fn compare(
    session: &Session,
    live: Stand,
    before: &Before,
    policy: &Policy,
) -> Result<(Stand, Option<Sealed>), StandError> {
    let Some(sealed) = session.sealed()? else {
        return Ok((live, None));
    };
    let stand = read(&sealed, policy)?;
    refuse_changed(&live, &stand, before)?;
    Ok((stand, Some(sealed)))
}

/// `spec::load` on the copy: the seal compares with the same code that reads a recipe.
fn read(sealed: &Sealed, policy: &Policy) -> Result<Stand, StandError> {
    Ok(spec::load(&sealed.dir.join(STAND_FILE), policy)?)
}

fn refuse_changed(live: &Stand, sealed: &Stand, before: &Before) -> Result<(), StandError> {
    let files = live.recipe.changed(&sealed.recipe);
    if files.is_empty() {
        return Ok(());
    }
    Err(StandError::RecipeChanged {
        files,
        before: match before {
            Before::Session => "session".to_owned(),
            Before::Ref(name) => name.clone(),
        },
    })
}

fn folder(stand: &Stand) -> &Utf8Path {
    stand
        .file
        .parent()
        .expect("a stand file lies in its folder")
}
