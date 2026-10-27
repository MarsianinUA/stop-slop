use std::io;
use std::path::Path;

use gitcmd::{Git, GitError};

use crate::RepoError;

/// A non-zero exit is `RepoError::Git`.
pub(crate) fn run(dir: &Path, args: &[&str]) -> Result<Vec<u8>, RepoError> {
    Git::at(dir).run(args).map_err(repo_error)
}

/// Exit code 1 is `None`.
pub(crate) fn run_found(dir: &Path, args: &[&str]) -> Result<Option<Vec<u8>>, RepoError> {
    Git::at(dir).run_found(args).map_err(repo_error)
}

pub(crate) fn run_with_input(
    dir: &Path,
    args: &[&str],
    input: &[u8],
) -> Result<Vec<u8>, RepoError> {
    Git::at(dir).run_with_input(args, input).map_err(repo_error)
}

pub(crate) fn repo_error(error: GitError) -> RepoError {
    match error {
        GitError::Failed { command, stderr } => RepoError::Git { command, stderr },
        GitError::Io(source) => RepoError::io("git")(source),
    }
}

/// A name that is not UTF-8 is skipped: `Utf8PathBuf` cannot hold it, and one odd file
/// should not fail a whole list.
pub(crate) fn split(stdout: &[u8], separator: u8) -> impl Iterator<Item = &str> {
    stdout
        .split(move |&byte| byte == separator)
        .filter(|piece| !piece.is_empty())
        .filter_map(|piece| str::from_utf8(piece).ok())
}

pub(crate) fn unexpected(command: &str) -> RepoError {
    RepoError::io("git")(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("unexpected output of `git {command}`"),
    ))
}
