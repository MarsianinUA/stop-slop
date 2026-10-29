//! `snapshot`: the whole work tree, uncommitted files too, as the "before" of this session (plan,
//! rows 37, 50). No lock and no VM: the ref is the work tree's own.

use std::time::SystemTime;

use crate::error::StandError;
use crate::{Env, Exit};

pub(crate) fn run(env: &mut Env<'_>) -> Result<Exit, StandError> {
    let worktree = source::open(env.cwd)?;
    let tree = source::snapshot(&worktree)?;
    let pinned = source::pin(&worktree, &tree, SystemTime::now())?;
    let out = StandError::io("stdout");
    writeln!(env.out, "{}", pinned.commit).map_err(&out)?;
    if let Some(old) = pinned.replaced {
        writeln!(env.out, "replaced {old}").map_err(&out)?;
    }
    Ok(Exit::Passed)
}
