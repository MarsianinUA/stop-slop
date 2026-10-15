use state::State;

use crate::error::{CliError, Exit};

pub(crate) fn enable() -> Result<Exit, CliError> {
    let repo = crate::cwd_repo()?;
    State::open(repo.git_common_dir()).enable()?;
    Ok(Exit::Clean)
}

pub(crate) fn disable() -> Result<Exit, CliError> {
    let repo = crate::cwd_repo()?;
    State::open(repo.git_common_dir()).disable()?;
    Ok(Exit::Clean)
}
