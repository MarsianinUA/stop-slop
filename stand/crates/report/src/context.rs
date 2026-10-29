//! What only `cli` knows about a run: it builds the `Context` and copies nothing of `Raw` (plan,
//! row 85).

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
