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
    #[error(".git/info/stop-slop/config.toml: {0}")]
    Config(#[from] ConfigError),
    #[error("fingerprint `{0}` is not among the findings of the branch")]
    UnknownFingerprint(String),
    #[error("hook input: {0}")]
    HookInput(String),
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Exit {
    Clean = 0,
    Found = 1,
    Failure = 2,
}
