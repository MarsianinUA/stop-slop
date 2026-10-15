use std::panic::resume_unwind;
use std::path::PathBuf;
use std::thread;

use camino::Utf8PathBuf;
use repo::{LineRange, Oid, Repo, RepoError, Snapshot};

mod base;
mod files;
mod pr_base;

pub use pr_base::{GhPrBase, PrBase};

#[derive(Debug)]
pub enum ScopeRequest {
    /// `base` is a name or a sha given by the user; without it the base is searched.
    Branch {
        base: Option<String>,
    },
    /// Paths of the user, absolute or relative to the `cwd` given to `Repo::open`.
    Files(Vec<PathBuf>),
    All,
}

/// `base` is `None` and `skipped` is empty for `Files` and `All`.
#[derive(Debug, PartialEq, Eq)]
pub struct Scope {
    pub base: Option<Base>,
    pub files: Vec<ScopedFile>,
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Base {
    pub reference: String,
    pub oid: Oid,
    pub reason: BaseReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseReason {
    Flag,
    Pr,
    Ancestor,
    Default,
}

/// `changed` is `None` for `Files` and `All`: every line counts.
#[derive(Debug, PartialEq, Eq)]
pub struct ScopedFile {
    pub path: Utf8PathBuf,
    pub changed: Option<Vec<LineRange>>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Skipped {
    pub path: Utf8PathBuf,
    pub reason: SkipReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The file is the same as in the tip of the base: a squash merge brought it there.
    AlreadyInBase,
}

#[derive(Debug, thiserror::Error)]
pub enum ScopeError {
    #[error(transparent)]
    Repo(#[from] RepoError),
    #[error("base `{0}` is not a commit")]
    UnknownBase(String),
    #[error("no base branch: no pull request, no ancestor branch and no origin/HEAD; pass --base")]
    NoBase,
    #[error("a revision needs a base")]
    RevWithoutBase,
}

/// The base and the files to check. Only `.ts`, `.tsx`, `.mts` and `.cts` files, without
/// declaration files; the paths are sorted.
pub fn resolve(
    repo: &Repo,
    snapshot: &Snapshot,
    pr: &dyn PrBase,
    request: ScopeRequest,
) -> Result<Scope, ScopeError> {
    let paths = match request {
        ScopeRequest::Branch { base } => {
            let base = base::find(repo, snapshot, pr, base)?;
            let (files, skipped) = files::of_branch(snapshot, &base.oid)?;
            return Ok(Scope {
                base: Some(base),
                files,
                skipped,
            });
        }
        ScopeRequest::Files(paths) => paths
            .iter()
            .map(|path| repo.relativize(path))
            .collect::<Result<_, _>>()?,
        ScopeRequest::All => snapshot.files()?,
    };
    Ok(Scope {
        base: None,
        files: files::listed(paths),
        skipped: Vec::new(),
    })
}

fn both<A: Send, B>(spawned: impl FnOnce() -> A + Send, here: impl FnOnce() -> B) -> (A, B) {
    thread::scope(|scope| {
        let spawned = scope.spawn(spawned);
        let here = here();
        let spawned = spawned.join().unwrap_or_else(|panic| resume_unwind(panic));
        (spawned, here)
    })
}
