use engine::ConfigError;
use project::ProjectError;
use repo::RepoError;
use state::StateError;

#[derive(Debug, thiserror::Error)]
pub(crate) enum CliError {
    #[error(transparent)]
    Repo(#[from] RepoError),
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error(transparent)]
    State(#[from] StateError),
    /// `ConfigError` names the key, not the file.
    #[error(".git/info/stop-slop/config.toml: {0}")]
    Config(#[from] ConfigError),
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Exit {
    Clean = 0,
    Failure = 2,
}
