//! `snapshot`: the whole work tree, uncommitted files too, as the "before" of this session (plan,
//! rows 37, 50). No lock and no VM: the ref is the work tree's own.

use std::io::Write;
use std::time::SystemTime;

use camino::Utf8Path;

use crate::Exit;
use crate::error::StandError;

pub(crate) fn run(cwd: &Utf8Path, out: &mut dyn Write) -> Result<Exit, StandError> {
    let worktree = source::open(cwd)?;
    let tree = source::snapshot(&worktree)?;
    let pinned = source::pin(&worktree, &tree, SystemTime::now())?;
    let stdout = StandError::io("stdout");
    writeln!(out, "{}", pinned.commit).map_err(&stdout)?;
    if let Some(old) = pinned.replaced {
        writeln!(out, "replaced {old}").map_err(&stdout)?;
    }
    Ok(Exit::Passed)
}
