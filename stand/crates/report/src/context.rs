//! What only `cli` knows about a run: it builds the `Context` and copies nothing of `Raw` (plan,
//! row 85).

use std::fmt;
use std::time::SystemTime;

use camino::Utf8PathBuf;

use crate::{Previous, Report};

#[derive(Debug)]
pub struct Context<'r> {
    pub run: String,
    pub created_at: SystemTime,
    pub root: Utf8PathBuf,
    pub head_tree: String,
    /// `None` without `--before`.
    pub before: Option<BeforeInfo>,
    /// The seal this run compared under; `None` without `--before`.
    pub seal: Option<SealInfo>,
    /// The earlier runs of the session, oldest first.
    pub history: Vec<PastRun>,
    /// `expect.cost` of the recipe: a measure without an entry is `none` (plan, row 66).
    pub declared: Vec<Declared>,
    /// The session's acceptances, oldest first (plan, row 67).
    pub accepted: Vec<Declared>,
    pub previous: Previous<'r>,
    /// The passed reports with this run's `PoolKey`, oldest first; their raw samples join this
    /// run's.
    pub pool: &'r [Report],
    /// The run that built each version's cached setup (plan, row 88).
    pub setup_from: SetupFrom,
}

/// `None`: this run built that setup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetupFrom {
    pub head: Option<String>,
    pub before: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeforeInfo {
    pub intent: Intent,
    /// What the intent resolved to: the snapshot commit of `session`, the merge-base of a ref.
    pub treeish: String,
    pub tree: String,
    /// The time of the snapshot for `session`, the commit date of the merge-base for a ref.
    pub taken_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    Session,
    Ref(String),
}

/// The recipe's seal (plan, row 65).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealInfo {
    /// Of the last seal or reseal: the comparisons count from it.
    pub sealed_at: SystemTime,
    /// `max_tolerance` of `stand.toml`, in percent.
    pub max_tolerance: u16,
    /// Oldest first; the entries outlive `clean`.
    pub journal: Vec<Journal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Journal {
    Sealed {
        sealed_at: SystemTime,
        digest: String,
    },
    Resealed {
        resealed_at: SystemTime,
        digest: String,
        reason: String,
    },
    Cleaned {
        cleaned_at: SystemTime,
    },
}

/// `compared`: the run had `--before`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PastRun {
    pub run: String,
    pub created_at: SystemTime,
    pub head_tree: String,
    pub compared: bool,
    pub passed: bool,
}

/// From `expect.cost` with `post_hoc: None`, or from `accept` with `post_hoc: Some`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declared {
    pub measure: String,
    pub bound: Bound,
    pub reason: String,
    pub post_hoc: Option<PostHoc>,
}

/// An acceptance holds only for the head tree it was made on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostHoc {
    pub head_tree: String,
    pub accepted_at: SystemTime,
}

/// `UpTo` and `DownTo` in whole percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    None,
    UpTo(u16),
    DownTo(u16),
    Down,
    Unknown,
}

/// The grammar of `expect.cost[].change` and of `accept --allow`: `none`, `+10%`, `-10%`,
/// `down`, `unknown`.
impl fmt::Display for Bound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Bound::None => f.write_str("none"),
            Bound::UpTo(percent) => write!(f, "+{percent}%"),
            Bound::DownTo(percent) => write!(f, "-{percent}%"),
            Bound::Down => f.write_str("down"),
            Bound::Unknown => f.write_str("unknown"),
        }
    }
}
