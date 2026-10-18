use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
#[error("{}{}: {problem}", .rule.as_ref().map_or(String::new(), |id| format!("rule `{id}`, ")), .path.display())]
pub struct FolderError {
    pub rule: Option<String>,
    pub path: PathBuf,
    pub problem: Problem,
}

impl FolderError {
    pub(crate) fn new(rule: Option<&str>, path: &Path, problem: Problem) -> Self {
        Self {
            rule: rule.map(str::to_string),
            path: path.to_path_buf(),
            problem,
        }
    }
}

pub(crate) fn io(error: io::Error) -> Problem {
    Problem::Io(error.to_string())
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum Problem {
    #[error("bad TOML: {0}")]
    BadToml(String),
    #[error("unknown key `{0}`")]
    UnknownKey(String),
    #[error("the folder name is not a kebab-case id")]
    BadId,
    #[error("another group has a rule with this id")]
    DuplicateId,
    #[error("the id is a Rust keyword, so it cannot name a module")]
    RustKeyword,
    #[error("an agent rule has check.rs")]
    AgentHasCode,
    #[error("an agent rule has fixtures")]
    AgentHasFixtures,
    #[error("an agent rule has `fix = true`")]
    AgentHasFix,
    #[error("an agent rule needs a non-empty `agent` hint")]
    AgentNoHint,
    #[error("check.rs is missing")]
    MissingCheck,
    #[error("valid.ts or invalid.ts is missing")]
    MissingFixtures,
    #[error("`fix = true` needs fixed.ts, and fixed.ts needs `fix = true`")]
    FixMismatch,
    #[error("no `// expect:` mark")]
    NoExpect,
    #[error("`// expect:` names another rule `{0}`")]
    ForeignExpect(String),
    #[error("unexpected file `{0}`")]
    ExtraFile(String),
    #[error("unknown source `{0}`")]
    UnknownSource(String),
    /// The file cannot be read or is not UTF-8.
    #[error("cannot read: {0}")]
    Io(String),
    /// A malformed mark: `// expect:` without an id, `x0`, a `// @...` header without a value.
    #[error("bad fixture mark: {0}")]
    BadFixture(String),
}
