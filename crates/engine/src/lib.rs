mod config;
mod dispatch;
mod finding;
mod run;
mod text;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use camino::Utf8PathBuf;
use kit::{DynRule, FindingKind, Position, RuleKind, RuleMeta, Ruleset};

pub use config::ConfigError;
use dispatch::Dispatch;

/// The same fields as `config.toml` and `ignore.json`.
#[derive(Debug, Default)]
pub struct RunConfig {
    pub disabled: Vec<String>,
    /// `[rules.<id>]` of `config.toml`.
    pub options: BTreeMap<String, toml::Table>,
    /// (file, fingerprint) pairs of `ignore.json`.
    pub ignored: BTreeSet<(Utf8PathBuf, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Check,
    /// Runs only `kind = error` rules.
    Hook,
}

impl Mode {
    fn runs(self, kind: RuleKind) -> bool {
        self == Mode::Check || kind == RuleKind::Error
    }
}

#[derive(Debug)]
pub struct SourceFile {
    pub path: Utf8PathBuf,
    pub bytes: Vec<u8>,
    /// `None` when there is no base to compare with.
    pub changed: Option<Vec<LineRange>>,
}

/// Lines from 1, both ends included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug)]
pub struct RunReport {
    /// Sorted by file, line, column, rule.
    pub findings: Vec<Finding>,
}

#[derive(Debug)]
pub struct Finding {
    /// `"parse-error"` when the file did not parse.
    pub rule: &'static str,
    pub kind: FindingKind,
    pub file: Utf8PathBuf,
    pub start: Position,
    pub end: Position,
    pub message: String,
    /// `None` when there is no base; otherwise whether the first line of the finding changed.
    pub in_hunk: Option<bool>,
    pub fixable: bool,
    pub fingerprint: Fingerprint,
}

/// `<rule>:<hash6>` of the finding's text and the name of the function around it, so it
/// survives lines added above. It is stored in `ignore.json`: the hash must not change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint(String);

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug)]
pub struct RuleState {
    pub meta: &'static RuleMeta,
    pub active: bool,
}

pub struct Engine {
    set: &'static Ruleset,
    /// By index in `set.rules`; `agent` rules too.
    enabled: Vec<bool>,
    /// The enabled `Impl::Check` rules, built, with their index in `set.rules`.
    rules: Vec<(usize, Box<dyn DynRule>)>,
    dispatch: Dispatch,
    ignored: BTreeSet<(Utf8PathBuf, String)>,
}
