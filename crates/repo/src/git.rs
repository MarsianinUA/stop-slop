use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;

use crate::RepoError;

/// Set by a git hook or the user's shell, these point git at another repo, index or object
/// store than the one of `cwd`, or make the `*.ts` pathspecs match literally, without
/// crossing `/` or ignoring case. `GIT_DIFF_OPTS` beats `-U0` and would turn context lines
/// into changed ones.
const CLEARED_ENV: [&str; 11] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_LITERAL_PATHSPECS",
    "GIT_GLOB_PATHSPECS",
    "GIT_NOGLOB_PATHSPECS",
    "GIT_ICASE_PATHSPECS",
    "GIT_DIFF_OPTS",
];

/// A non-zero exit is `RepoError::Git`.
pub(crate) fn run(dir: &Path, args: &[&str]) -> Result<Vec<u8>, RepoError> {
    succeeded(args, output(dir, args, None)?)
}

/// Exit code 1 is `None`: `grep`, `merge-base`, `symbolic-ref --quiet` and
/// `rev-parse --quiet` use it for "nothing found".
pub(crate) fn run_found(dir: &Path, args: &[&str]) -> Result<Option<Vec<u8>>, RepoError> {
    let output = output(dir, args, None)?;
    match output.status.code() {
        Some(0) => Ok(Some(output.stdout)),
        Some(1) => Ok(None),
        _ => Err(failed(args, &output.stderr)),
    }
}

pub(crate) fn run_with_input(
    dir: &Path,
    args: &[&str],
    input: &[u8],
) -> Result<Vec<u8>, RepoError> {
    succeeded(args, output(dir, args, Some(input))?)
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

fn output(dir: &Path, args: &[&str], input: Option<&[u8]>) -> Result<Output, RepoError> {
    let mut command = Command::new("git");
    for name in CLEARED_ENV {
        command.env_remove(name);
    }
    let mut child = command
        .args(args)
        .current_dir(dir)
        // English messages: `Repo::open` recognizes "not a git repository" in stderr.
        .env("LC_ALL", "C")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(RepoError::io("git"))?;
    let stdin = child.stdin.take();
    thread::scope(|scope| {
        // A long input is written by its own thread while this one reads stdout: otherwise
        // both pipes fill up and git and we wait for each other forever.
        if let (Some(mut stdin), Some(input)) = (stdin, input) {
            scope.spawn(move || {
                // A failed write means git closed its stdin; its exit code and output tell why.
                let _ = stdin.write_all(input);
            });
        }
        child.wait_with_output()
    })
    .map_err(RepoError::io("git"))
}

fn succeeded(args: &[&str], output: Output) -> Result<Vec<u8>, RepoError> {
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(failed(args, &output.stderr))
    }
}

fn failed(args: &[&str], stderr: &[u8]) -> RepoError {
    RepoError::Git {
        command: format!("git {}", args.join(" ")),
        stderr: String::from_utf8_lossy(stderr).trim_end().to_owned(),
    }
}
