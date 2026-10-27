use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::Duration;

/// Before the second and the third start of a git that could not open the index.
const INDEX_RETRY_PAUSES: [Duration; 2] = [Duration::from_millis(10), Duration::from_millis(50)];

/// Set by a git hook or the user's shell, these point git at another repo, index or object
/// store than the one of the folder, or make pathspecs like `*.ts` match literally, without
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

#[derive(Debug)]
pub struct Git {
    dir: PathBuf,
    overrides: Vec<(String, OsString)>,
}

impl Git {
    pub fn at(dir: &Path) -> Git {
        Git {
            dir: dir.to_owned(),
            overrides: Vec::new(),
        }
    }

    /// Set after the cleared variables are removed: a `GIT_INDEX_FILE` given here reaches git.
    pub fn env(mut self, name: &str, value: impl AsRef<OsStr>) -> Git {
        self.overrides
            .push((name.to_owned(), value.as_ref().to_owned()));
        self
    }

    /// A non-zero exit is `GitError::Failed`.
    pub fn run(&self, args: &[&str]) -> Result<Vec<u8>, GitError> {
        succeeded(args, self.output(args, None)?)
    }

    /// Exit code 1 is `None`: `grep`, `merge-base`, `symbolic-ref --quiet` and
    /// `rev-parse --quiet` use it for "nothing found".
    pub fn run_found(&self, args: &[&str]) -> Result<Option<Vec<u8>>, GitError> {
        let output = self.output(args, None)?;
        match output.status.code() {
            Some(0) => Ok(Some(output.stdout)),
            Some(1) => Ok(None),
            _ => Err(failed(args, &output.stderr)),
        }
    }

    pub fn run_with_input(&self, args: &[&str], input: &[u8]) -> Result<Vec<u8>, GitError> {
        succeeded(args, self.output(args, Some(input))?)
    }

    /// On Windows `git diff` on the work tree rewrites the index itself
    /// (`diff.autoRefreshIndex`) when a file's stat changed but not its content. A git started
    /// at that moment, ours in parallel or an IDE's, cannot open the index and fails; a moment
    /// later it would not.
    fn output(&self, args: &[&str], input: Option<&[u8]>) -> Result<Output, GitError> {
        let mut output = self.run_once(args, input)?;
        for pause in INDEX_RETRY_PAUSES {
            if output.status.success() || !index_busy(&output.stderr) {
                break;
            }
            thread::sleep(pause);
            output = self.run_once(args, input)?;
        }
        Ok(output)
    }

    fn run_once(&self, args: &[&str], input: Option<&[u8]>) -> Result<Output, GitError> {
        let mut command = Command::new("git");
        for name in CLEARED_ENV {
            command.env_remove(name);
        }
        // English messages: `root` and the callers recognize git's errors in stderr.
        command.env("LC_ALL", "C");
        for (name, value) in &self.overrides {
            command.env(name, value);
        }
        let mut child = command
            .args(args)
            .current_dir(&self.dir)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(GitError::Io)?;
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
        .map_err(GitError::Io)
    }
}

/// Absolute, as git prints them: `C:/...` on Windows, never `\\?\C:\...`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub worktree: PathBuf,
    pub common: PathBuf,
}

/// `None` outside a git repository.
pub fn root(dir: &Path) -> Result<Option<Root>, GitError> {
    // From a subfolder git prints the common dir relative to `dir`, which may be relative
    // itself; `--path-format` makes it absolute. The root is absolute anyway.
    let args = [
        "rev-parse",
        "--show-toplevel",
        "--path-format=absolute",
        "--git-common-dir",
    ];
    let out = match Git::at(dir).run(&args) {
        Err(GitError::Failed { stderr, .. }) if stderr.contains("not a git repository") => {
            return Ok(None);
        }
        other => other?,
    };
    let (worktree, common) = str::from_utf8(&out)
        .ok()
        .and_then(|text| text.strip_suffix('\n')?.split_once('\n'))
        .ok_or_else(|| {
            GitError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected output of `git rev-parse`",
            ))
        })?;
    Ok(Some(Root {
        worktree: PathBuf::from(worktree),
        common: PathBuf::from(common),
    }))
}

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("`{command}` failed: {stderr}")]
    Failed { command: String, stderr: String },
    #[error("git: {0}")]
    Io(#[source] io::Error),
}

fn index_busy(stderr: &[u8]) -> bool {
    const BUSY: &[u8] = b"index file open failed";
    stderr.windows(BUSY.len()).any(|window| window == BUSY)
}

fn succeeded(args: &[&str], output: Output) -> Result<Vec<u8>, GitError> {
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(failed(args, &output.stderr))
    }
}

fn failed(args: &[&str], stderr: &[u8]) -> GitError {
    GitError::Failed {
        command: format!("git {}", args.join(" ")),
        stderr: String::from_utf8_lossy(stderr).trim_end().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_failed_index_open_is_started_again() {
        assert!(index_busy(
            b"fatal: .git/index: index file open failed: Permission denied\n"
        ));
        assert!(!index_busy(
            b"fatal: not a git repository (or any of the parent directories): .git\n"
        ));
        assert!(!index_busy(b"fatal: bad revision 'no-such'\n"));
        assert!(!index_busy(b""));
    }
}
