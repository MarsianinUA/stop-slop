use std::path::PathBuf;

use engine::ConfigError;
use project::ProjectError;
use repo::RepoError;
use scope::ScopeError;
use state::StateError;

#[derive(Debug, thiserror::Error)]
pub(crate) enum CliError {
    #[error(transparent)]
    Repo(#[from] RepoError),
    #[error(transparent)]
    Scope(#[from] ScopeError),
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error(transparent)]
    State(#[from] StateError),
    /// `ConfigError` names the key, not the file.
    #[error("{}: {source}", .path.display())]
    Config {
        path: PathBuf,
        #[source]
        source: ConfigError,
    },
    #[error("fingerprint `{0}` is not among the findings of the branch")]
    UnknownFingerprint(String),
    #[error("hook input: {0}")]
    HookInput(String),
}

impl CliError {
    pub(crate) fn exit(&self) -> Exit {
        match self {
            CliError::Repo(error)
            | CliError::Scope(ScopeError::Repo(error))
            | CliError::Project(ProjectError::Repo(error)) => match error {
                RepoError::NotARepo | RepoError::OutsideRepo(_) | RepoError::UnknownRev(_) => {
                    Exit::Refused
                }
                RepoError::Git { .. } | RepoError::Io { .. } => Exit::Broken,
            },
            CliError::Scope(
                ScopeError::UnknownBase(_)
                | ScopeError::NoBase
                | ScopeError::RevWithoutBase
                | ScopeError::BadFile { .. },
            )
            | CliError::Project(ProjectError::Json { .. })
            | CliError::State(StateError::Parse { .. })
            | CliError::Config { .. }
            | CliError::UnknownFingerprint(_)
            | CliError::HookInput(_) => Exit::Refused,
            CliError::State(StateError::Io { .. }) => Exit::Broken,
        }
    }

    /// `failure.phase` of the `--json` line on 2 and 3.
    pub(crate) fn phase(&self) -> &'static str {
        match self {
            CliError::Repo(error) | CliError::Scope(ScopeError::Repo(error)) => match error {
                RepoError::NotARepo
                | RepoError::OutsideRepo(_)
                | RepoError::UnknownRev(_)
                | RepoError::Git { .. }
                | RepoError::Io { .. } => "repo",
            },
            CliError::Scope(
                ScopeError::UnknownBase(_) | ScopeError::NoBase | ScopeError::RevWithoutBase,
            ) => "base",
            CliError::Scope(ScopeError::BadFile { .. }) => "files",
            CliError::Project(_) => "project",
            CliError::Config { .. } => "config",
            CliError::State(_) => "state",
            CliError::UnknownFingerprint(_) => "fingerprint",
            // The hook takes its input on stdin instead of flags.
            CliError::HookInput(_) => "args",
        }
    }

    /// The commands of the `next:` lines and of `next`; empty when no command helps.
    pub(crate) fn next(&self) -> Vec<String> {
        match self {
            CliError::Scope(ScopeError::UnknownBase(_) | ScopeError::NoBase) => {
                vec!["stop-slop check --base <ref> --json".to_owned()]
            }
            CliError::UnknownFingerprint(_) => vec!["stop-slop check --json".to_owned()],
            CliError::Repo(error) | CliError::Scope(ScopeError::Repo(error)) => match error {
                RepoError::NotARepo
                | RepoError::OutsideRepo(_)
                | RepoError::UnknownRev(_)
                | RepoError::Git { .. }
                | RepoError::Io { .. } => Vec::new(),
            },
            CliError::Scope(ScopeError::RevWithoutBase | ScopeError::BadFile { .. })
            | CliError::Project(_)
            | CliError::Config { .. }
            | CliError::State(_)
            | CliError::HookInput(_) => Vec::new(),
        }
    }
}

/// `Refused` is about the input: a flag, a file of the user, a name. `Broken` is the program
/// or what it runs on: git, the disk.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Exit {
    Clean = 0,
    Found = 1,
    Refused = 2,
    Broken = 3,
    /// Ctrl+C: 128 and the number of `SIGINT`, as a shell reports it.
    Interrupted = 130,
}
