//! The one translation of the errors of the stand into exit codes, and the next steps of each
//! (plan, «Visible errors `run`», rows 4, 83). The phase of a failed `run` is not here: the step
//! that failed names it (`Halt`), since one `StoreError` or `SandboxError` can happen in several.

use std::io;
use std::time::Duration;

use budget::{HostError, Refusal};
use camino::Utf8PathBuf;
use report::{Next, Phase, ReportError, Topic};
use run::{InspectError, StandFault};
use sandbox::SandboxError;
use source::SourceError;
use spec::{ChangeError, SpecError};
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
    /// `cli` also refuses a run id other than the kept run's as `NotKept`.
    #[error(transparent)]
    Inspect(#[from] InspectError),
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
    #[error("this work tree has no passed run to accept a measure of")]
    NoRun,
    #[error("`{measure}` is not a measure of the last passed run, {run}")]
    NotInReport { measure: String, run: String },
    #[error(transparent)]
    Change(#[from] ChangeError),
    #[error("`unknown` bounds nothing: accept none, +X%, -X% or down")]
    UnknownBound,
    #[error("{allow} is wider than max_tolerance, {max}%: the measure needs a person")]
    TooWide { allow: String, max: u16 },
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
            StandError::Inspect(error) => match error {
                InspectError::NotKept(_) | InspectError::NotStarted { .. } => Exit::Refused,
                InspectError::Stand(_) => Exit::Broken,
                InspectError::Cancelled => Exit::Cancelled,
            },
            StandError::Config(_)
            | StandError::Budget(_)
            | StandError::LockWait(_)
            | StandError::RecipeChanged { .. }
            | StandError::NothingToCompare
            | StandError::NoRun
            | StandError::NotInReport { .. }
            | StandError::Change(_)
            | StandError::UnknownBound
            | StandError::TooWide { .. } => Exit::Refused,
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
            // The topic says which run, side and service `inspect` takes, and how its copy starts.
            StandError::Inspect(InspectError::NotKept(_) | InspectError::NotStarted { .. }) => {
                vec![Next::Guide(Some(Topic::Inspect))]
            }
            // The topic holds the grammar of a bound and when `accept` applies.
            StandError::NoRun
            | StandError::NotInReport { .. }
            | StandError::Change(_)
            | StandError::UnknownBound => vec![Next::Guide(Some(Topic::Expect))],
            // A bound wider than the policy is for a person, not a command.
            StandError::TooWide { .. }
            | StandError::Cancelled
            | StandError::Inspect(InspectError::Cancelled) => Vec::new(),
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
