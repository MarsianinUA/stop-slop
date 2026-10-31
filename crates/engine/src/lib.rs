mod config;
mod dispatch;
mod finding;
mod fix;
mod functions;
mod place;
mod run;
mod text;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use camino::Utf8PathBuf;
use kit::{DynRule, FindingKind, Position, RuleDef, RuleKind, RuleMeta, Ruleset};

pub use config::ConfigError;
use dispatch::Dispatch;

/// The same fields as `config.toml` and `ignore.json`.
#[derive(Debug, Default)]
pub struct RunConfig {
    pub disabled: Vec<String>,
    /// Opt-in groups to turn on.
    pub enabled: Vec<String>,
    /// `[rules.<id>]` of `config.toml`.
    pub options: BTreeMap<String, toml::Table>,
    /// (file, fingerprint) pairs of `ignore.json`.
    pub ignored: BTreeSet<(Utf8PathBuf, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Check,
    /// Runs only `fix = true` rules and applies their edits, except on ignored findings. It
    /// reports no findings: a `Check` run on the written files shows what is left.
    Fix,
    /// Runs only `kind = error` rules.
    Hook,
}

impl Mode {
    /// In the order of the variants, so `Mode::ALL[mode as usize] == mode`.
    const ALL: [Mode; 3] = [Mode::Check, Mode::Fix, Mode::Hook];

    fn runs(self, meta: &RuleMeta) -> bool {
        match self {
            Mode::Check => true,
            Mode::Fix => meta.fix,
            Mode::Hook => meta.kind == RuleKind::Error,
        }
    }
}

// `Dispatch` builds profiles in the order of `Mode::ALL` and picks one by `mode as usize`.
const _: () = {
    let mut i = 0;
    while i < Mode::ALL.len() {
        assert!(
            Mode::ALL[i] as usize == i,
            "Mode::ALL lists the variants out of order"
        );
        i += 1;
    }
};

#[derive(Debug)]
pub struct SourceFile {
    pub path: Utf8PathBuf,
    pub bytes: Vec<u8>,
    /// `None` when there is no base to compare with.
    pub changed: Option<Changed>,
}

/// What the diff did to a file.
#[derive(Debug, Clone, Default)]
pub struct Changed {
    pub lines: Vec<LineRange>,
    /// The hunks that took out a line of code.
    pub cuts: Vec<Cut>,
    /// The diff adds the file or renames it.
    pub new_path: bool,
}

/// Lines from 1, both ends included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}

/// A hunk on its new side: `count` lines from `start` stand where the old lines were, or with
/// `count = 0` the old lines went out after line `start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cut {
    pub start: u32,
    pub count: u32,
}

#[derive(Debug)]
pub struct RunReport {
    /// Sorted by file, line, column, rule; empty in `Fix`.
    pub findings: Vec<Finding>,
    /// Only in `Fix`; sorted by file, rule.
    pub fixed: Vec<FixCount>,
    /// Only in `Fix`; sorted by path.
    pub rewritten: Vec<Rewritten>,
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
    /// The values of the `{placeholders}` of the rule's message, in the order the rule gave
    /// them; for a `parse-error` its `error`.
    pub args: Vec<(&'static str, String)>,
    /// `None` when there is no base; otherwise whether the first line of the finding changed.
    pub in_hunk: Option<bool>,
    /// The innermost function or class member around the finding, else its top-level
    /// statement, as the code names it: `Class.method`, an arrow by its variable,
    /// `(anonymous)`, `(top level)` outside any declaration. `None` for a finding about the
    /// file itself: its path, or a text that does not parse.
    pub function: Option<String>,
    /// `None` when there is no base. Otherwise whether the diff changed a line of code of
    /// `function`, a comment or a blank line not counted, or took code out of it; for a
    /// finding about the path, whether it added or renamed the file; for a `parse-error`,
    /// whether it changed any line of the file or took code out of it.
    pub touched: Option<bool>,
    pub fixable: bool,
    pub fingerprint: Fingerprint,
}

/// A function or arrow of a file, from `Engine::functions`.
#[derive(Debug, PartialEq, Eq)]
pub struct FunctionSpan {
    /// Its own name, or for an anonymous one the name of the function around it, as a
    /// fingerprint takes it; `None` when no function around has one.
    pub name: Option<String>,
    pub start: Position,
    /// Just past its last character.
    pub end: Position,
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

/// How many findings of `rule` `Fix` fixed in `file`.
#[derive(Debug, PartialEq, Eq)]
pub struct FixCount {
    pub rule: &'static str,
    pub file: Utf8PathBuf,
    pub count: u32,
}

/// The new content of a file `Fix` changed, with its BOM and line endings as they were.
#[derive(Debug)]
pub struct Rewritten {
    pub path: Utf8PathBuf,
    pub bytes: Vec<u8>,
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
    /// The enabled `Impl::Check` rules, built.
    rules: Vec<(&'static RuleDef, Box<dyn DynRule>)>,
    dispatch: Dispatch,
    ignored: BTreeSet<(Utf8PathBuf, String)>,
}
