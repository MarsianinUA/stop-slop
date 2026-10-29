//! `report.json`, `schemaVersion` 1, and the `--json` line (architecture plan, «report.json»): keys
//! camelCase, values kebab-case, every path with `/`, oids in full. `write` and `load` are the only
//! I/O of the report after `read_outputs`.

use std::fs;
use std::time::{Duration, SystemTime};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::context::{BeforeInfo, Intent, SetupFrom};
use crate::flags::Flag;
use crate::next::Next;
use crate::outputs::{RECORDS, Records};
use crate::raw::Failure;
use crate::{Before, ReportError};

pub(crate) const SCHEMA_VERSION: u32 = 1;

pub(crate) const REPORT: &str = "report.json";

pub(crate) fn diff_path(dir: &Utf8Path, file: &str) -> Utf8PathBuf {
    dir.join("diff").join(format!("{file}.json"))
}

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
    /// By `selfMs`, the hottest first.
    pub hot: Vec<Hot>,
    pub coverage: Option<CoverageJson>,
    /// `None` without `--before` or when "before" failed.
    pub behaviour: Option<Vec<Behaviour>>,
    /// This run's records for `records.json`; a loaded report has none.
    #[serde(skip)]
    pub(crate) records: Records,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hot {
    pub service: String,
    /// From the repo root, with `/`.
    pub file: String,
    /// V8's name; empty for an anonymous function.
    pub name: String,
    /// From 1, where V8 starts the function.
    pub line: u32,
    pub column: u32,
    /// Medians of the head's measured repeats, the replicas of the service added up.
    pub self_ms: f64,
    pub total_ms: f64,
    /// `None` without `--before`.
    pub before: Option<HotVersus>,
    pub previous: HotVersus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotVersus {
    pub verdict: Verdict,
}

/// From the coverage repeat only; `None` without one (`unprofiled`) or when it leaves the coverage
/// unknown (`coverage-unknown`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageJson {
    pub services: Vec<String>,
    /// Functions entered at least once, by file, line and column, the services added up.
    pub called: Vec<CalledJson>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalledJson {
    pub file: String,
    pub line: u32,
    pub column: u32,
    /// Entries into the body, not calls: each resumption after `await` counts too.
    pub calls: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Behaviour {
    /// Its name in `/stand/out`.
    pub file: String,
    pub src: Src,
    /// The probe's `key`; `None` pairs the records by position.
    pub key: Option<String>,
    /// At least two repeats of "before": with one, nothing is known to be noise.
    pub noise_known: bool,
    /// The paths that differ between repeats of "before": left out of `diffs`.
    pub noise: Vec<String>,
    /// The paths stable on "before" that differ between repeats of the head: kept in `diffs`.
    pub head_only_noise: Vec<String>,
    pub diffs: Vec<Diff>,
}

/// A record only on one side has `path` `""` and the other side `null`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diff {
    #[serde(flatten)]
    pub at: At,
    pub path: String,
    pub before: Value,
    pub head: Value,
}

/// `record` from 1 by position in the file, or the value at the probe's `key` instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum At {
    Record { record: u32 },
    Key { key: Value },
}

/// The quiet `--json` line (plan, row 76): `Report::line` or `Line::failed`. `cli` prints it with
/// `next` as command strings, and sets `run` of a failed run that has its folder.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub run: Option<String>,
    #[serde(serialize_with = "slash::option")]
    pub report: Option<Utf8PathBuf>,
    /// Only the exceptions.
    pub measures: Vec<LineMeasure>,
    /// Only the files with a diff or a flag that names them; `None` as in the report.
    pub behaviour: Option<Vec<LineFile>>,
    pub flags: Vec<Flag>,
    pub failure: Option<Failure>,
    /// The measures and files with no exception.
    pub quiet: u32,
    /// Measures without an `expect` entry graded `unmeasurable`: they stay quiet (plan, row 66).
    /// 0 until the grades of T-13b.
    pub unmeasurable: u32,
    #[serde(skip)]
    pub next: Vec<Next>,
}

/// A `Measure` without `direction`, `samples` and `spread`: `report.json` has them.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineMeasure {
    pub name: String,
    pub unit: Unit,
    pub src: Src,
    pub median: f64,
    pub before: Option<Compared>,
    pub previous: VersusPrevious,
}

/// `median` is `None` when "before" failed, as in `VersusBefore`.
#[derive(Debug, Clone, Serialize)]
pub struct Compared {
    pub median: Option<f64>,
    pub verdict: Verdict,
}

/// `sample`: at most 3 records of `diffs`; `path`: all of them, `diff/<file>.json` of the run folder.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineFile {
    pub file: String,
    pub src: Src,
    pub noise_known: bool,
    pub diffs: u32,
    pub sample: Vec<Diff>,
    #[serde(serialize_with = "slash::path")]
    pub path: Utf8PathBuf,
}

impl Report {
    /// `<dir>/diff/<file>.json` with all the diffs of each behaviour file and `<dir>/records.json`,
    /// then `<dir>/report.json` through a temporary file and a rename: a `report.json` is whole and
    /// says the rest is.
    pub fn write(&self, dir: &Utf8Path) -> Result<(), ReportError> {
        for file in self.behaviour.iter().flatten() {
            let path = diff_path(dir, &file.file);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(ReportError::io(parent))?;
            }
            write_json(&path, &file.diffs, false)?;
        }
        write_json(&dir.join(RECORDS), &self.records, false)?;
        let temporary = dir.join(format!("{REPORT}.tmp"));
        write_json(&temporary, self, true)?;
        let path = dir.join(REPORT);
        fs::rename(&temporary, &path).map_err(ReportError::io(&path))
    }
}

/// `pretty` for the files people read; the records only feed later pools.
fn write_json(path: &Utf8Path, value: &impl Serialize, pretty: bool) -> Result<(), ReportError> {
    let bytes = if pretty {
        serde_json::to_vec_pretty(value)
    } else {
        serde_json::to_vec(value)
    };
    let bytes = bytes.map_err(|error| ReportError::Bad {
        path: path.to_owned(),
        message: error.to_string(),
    })?;
    fs::write(path, bytes).map_err(ReportError::io(path))
}

/// `schemaVersion` first: a report of another version is `Schema`, not a parse error of its fields.
pub(crate) fn load(dir: &Utf8Path) -> Result<Report, ReportError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Version {
        schema_version: u64,
    }
    let path = dir.join(REPORT);
    let text = fs::read_to_string(&path).map_err(ReportError::io(&path))?;
    let bad = |error: serde_json::Error| ReportError::Bad {
        path: path.clone(),
        message: error.to_string(),
    };
    let version: Version = serde_json::from_str(&text).map_err(bad)?;
    if version.schema_version != u64::from(SCHEMA_VERSION) {
        return Err(ReportError::Schema {
            path,
            found: version.schema_version,
        });
    }
    serde_json::from_str(&text).map_err(bad)
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

/// `serialize_with` for the paths of `Failure`, `Line` and `LineFile`.
pub(crate) mod slash {
    use camino::{Utf8Path, Utf8PathBuf};
    use serde::Serializer;

    use super::slashed;

    pub(crate) fn path<S: Serializer>(path: &Utf8Path, out: S) -> Result<S::Ok, S::Error> {
        out.serialize_str(&slashed(path))
    }

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
