//! `report.json`, `schemaVersion` 1 (architecture plan, «report.json»): keys camelCase, values
//! kebab-case, every path with `/`, oids in full.

use std::time::{Duration, SystemTime};

use camino::Utf8Path;
use serde::{Deserialize, Serialize};

use crate::Before;
use crate::context::{BeforeInfo, Intent, SetupFrom};
use crate::flags::Flag;
use crate::raw::Failure;

pub(crate) const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub schema_version: u32,
    pub run: String,
    pub root: String,
    pub recipe: RecipeJson,
    pub created_at: String,
    pub repeat: u32,
    pub setup_timeout: String,
    pub timeout: String,
    pub harness: Vec<String>,
    pub head: Head,
    pub before: Option<BeforeJson>,
    pub previous_run: Option<String>,
    pub measures: Vec<Measure>,
    pub flags: Vec<Flag>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeJson {
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Head {
    pub tree: String,
    pub setup: SetupJson,
}

/// `cached`: the run that built the cached setup of the version, `None` when this run built it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupJson {
    pub cached: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeJson {
    pub intent: IntentJson,
    /// `None` for `session`.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub treeish: String,
    pub tree: String,
    pub taken_at: String,
    pub failed: Option<Failure>,
    pub setup: SetupJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IntentJson {
    Session,
    Ref,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Measure {
    /// `<subject>.<metric>` (plan, row 84).
    pub name: String,
    pub unit: Unit,
    pub direction: Direction,
    pub src: Src,
    /// This run's own, one per measured repeat; a volume has one, of its setup. Pooled runs keep
    /// theirs.
    pub samples: Vec<f64>,
    /// Of the samples of the pool and this run.
    pub median: f64,
    /// The median of the ranges of the attempts (plan, row 68).
    pub spread: f64,
    /// `None` without `--before`.
    pub before: Option<VersusBefore>,
    pub previous: VersusPrevious,
}

/// `median` is `None` when "before" failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersusBefore {
    pub samples: Vec<f64>,
    pub median: Option<f64>,
    pub verdict: Verdict,
}

/// `{median: null, verdict: no-baseline}` without a previous run or without the measure in it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersusPrevious {
    pub median: Option<f64>,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Unit {
    Ms,
    Bytes,
    Count,
    Share,
}

/// By metric, not by unit: `memory` and volume `bytes` share a unit but not a direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    Lower,
    Neutral,
}

/// `stand` on every measure; `probe` and `workload` on the behaviour files, which never become
/// measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Src {
    Stand,
    Probe,
    Workload,
}

/// `Changed` only for a `Neutral` measure, which is never `Improved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Improved,
    Regressed,
    Changed,
    WithinNoise,
    NoBaseline,
}

pub(crate) fn before_json(
    info: &BeforeInfo,
    before: Before<'_>,
    setup_from: &SetupFrom,
) -> BeforeJson {
    let (intent, reference) = match &info.intent {
        Intent::Session => (IntentJson::Session, None),
        Intent::Ref(name) => (IntentJson::Ref, Some(name.clone())),
    };
    BeforeJson {
        intent,
        reference,
        treeish: info.treeish.clone(),
        tree: info.tree.clone(),
        taken_at: time(info.taken_at),
        failed: match before {
            Before::Failed(failure) => Some(failure.clone()),
            Before::Absent | Before::Passed(_) => None,
        },
        setup: SetupJson {
            cached: setup_from.before.clone(),
        },
    }
}

/// RFC 3339 in UTC, to the second: `2026-10-22T10:00:00Z`.
pub(crate) fn time(at: SystemTime) -> String {
    humantime::format_rfc3339_seconds(at).to_string()
}

/// As the stand file writes it: `10m`, `1m30s`.
pub(crate) fn duration(duration: Duration) -> String {
    humantime::format_duration(duration)
        .to_string()
        .replace(' ', "")
}

/// Every path of the report and the line has `/`, on Windows too (plan, row 45); elsewhere a `\` is
/// part of a file name.
pub(crate) fn slashed(path: &Utf8Path) -> String {
    if cfg!(windows) {
        path.as_str().replace('\\', "/")
    } else {
        path.as_str().to_owned()
    }
}

/// `serialize_with` for the paths of `Failure`.
pub(crate) mod slash {
    use camino::Utf8PathBuf;
    use serde::Serializer;

    use super::slashed;

    pub(crate) fn option<S: Serializer>(
        path: &Option<Utf8PathBuf>,
        out: S,
    ) -> Result<S::Ok, S::Error> {
        match path {
            Some(path) => out.serialize_some(&slashed(path)),
            None => out.serialize_none(),
        }
    }

    pub(crate) fn paths<S: Serializer>(paths: &[Utf8PathBuf], out: S) -> Result<S::Ok, S::Error> {
        out.collect_seq(paths.iter().map(|path| slashed(path)))
    }
}
