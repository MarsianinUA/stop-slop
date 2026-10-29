//! The one translation of the errors of the stand into exit codes, and the next steps of each
//! (plan, «Visible errors `run`», rows 4, 83). The phase of a failed `run` is not here: the step
//! that failed names it (`Halt`), since one `StoreError` or `SandboxError` can happen in several.

use std::io;
use std::time::Duration;

use budget::{HostError, Refusal};
use camino::Utf8PathBuf;
use report::{Next, Phase, ReportError, Topic};
use run::StandFault;
use sandbox::SandboxError;
use source::SourceError;
use spec::SpecError;
use store::{ConfigError, StoreError};

use crate::Exit;

#[derive(Debug, thiserror::Error)]
pub(crate) enum StandError {
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Spec(#[from] SpecError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Sandbox(#[from] SandboxError),
    #[error(transparent)]
    Host(#[from] HostError),
    #[error("the stand does not fit the host: {0}")]
    Budget(#[from] Refusal),
    #[error(transparent)]
    Report(#[from] ReportError),
    /// A stand fault inside `run`, and the errors of `kept_ids` and `cached_ids`.
    #[error(transparent)]
    Run(#[from] StandFault),
    #[error("{path}: {source}")]
    Io {
        path: Utf8PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("another run holds the stand; waited {} s for it", .0.as_secs())]
    LockWait(Duration),
    /// `before`: the intent as typed, for the command that reseals.
    #[error(
        "the recipe changed since its seal: {}; put it back, or seal it again with --reseal",
        .files.join(", ")
    )]
    RecipeChanged { files: Vec<String>, before: String },
    #[error("the before tree is the head tree: there is nothing to compare")]
    NothingToCompare,
    #[error("cancelled")]
    Cancelled,
}

impl StandError {
    pub(crate) fn exit(&self) -> Exit {
        match self {
            StandError::Source(error) => match error {
                SourceError::NotARepo
                | SourceError::NoSnapshot
                | SourceError::UnknownRef(_)
                | SourceError::NoMergeBase(_) => Exit::Refused,
                SourceError::Git(_) | SourceError::Write(_) | SourceError::Io { .. } => {
                    Exit::Broken
                }
            },
            StandError::Spec(error) => match error {
                SpecError::Missing(_) | SpecError::Invalid { .. } => Exit::Refused,
                SpecError::Io { .. } => Exit::Broken,
            },
            StandError::Sandbox(error) => match error {
                SandboxError::ImageNotFound(_) => Exit::Refused,
                SandboxError::Registry { .. }
                | SandboxError::Gone(_)
                | SandboxError::Guest { .. }
                | SandboxError::Backend { .. } => Exit::Broken,
            },
            StandError::Config(_)
            | StandError::Budget(_)
            | StandError::LockWait(_)
            | StandError::RecipeChanged { .. }
            | StandError::NothingToCompare => Exit::Refused,
            StandError::Store(_)
            | StandError::Host(_)
            | StandError::Report(_)
            | StandError::Run(_)
            | StandError::Io { .. } => Exit::Broken,
            StandError::Cancelled => Exit::Cancelled,
        }
    }

    /// What helps next, as types; `next::render` turns them into commands.
    pub(crate) fn next(&self) -> Vec<Next> {
        match self {
            StandError::RecipeChanged { before, .. } => vec![Next::Reseal {
                before: before.clone(),
            }],
            StandError::Spec(SpecError::Missing(_)) => vec![Next::Init],
            StandError::Source(SourceError::NoSnapshot) => vec![Next::Snapshot],
            // The rules of the stand file, the image of the prepare service among them, live in
            // the topic `recipe` (plan, row 87).
            StandError::Spec(SpecError::Invalid { .. }) => vec![Next::Guide(Some(Topic::Recipe))],
            StandError::Cancelled => Vec::new(),
            _ => vec![Next::Guide(None)],
        }
    }
}

/// A failed step of `run`: the line's `failure.phase` and its cause.
#[derive(Debug)]
pub(crate) struct Halt {
    pub phase: Phase,
    pub error: StandError,
}

pub(crate) trait At<T> {
    fn at(self, phase: Phase) -> Result<T, Halt>;
}

impl<T, E: Into<StandError>> At<T> for Result<T, E> {
    fn at(self, phase: Phase) -> Result<T, Halt> {
        self.map_err(|error| Halt {
            phase,
            error: error.into(),
        })
    }
}
