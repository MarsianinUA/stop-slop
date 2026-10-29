//! `report.json`, `schemaVersion` 1, and the `--json` line (architecture plan, «report.json»): keys
//! camelCase, values kebab-case, every path with `/`, oids in full. `write` and `load` are the only
//! I/O of the report after `read_outputs`.

use std::fs;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::context::{BeforeInfo, Intent, Journal, SealInfo, SetupFrom};
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
    /// `None` without `--before`.
    pub seal: Option<SealJson>,
    pub created_at: String,
    pub repeat: u32,
    pub setup_timeout: String,
    pub timeout: String,
    /// The runs whose samples pool, this one included (plan, row 68).
    pub attempts: u32,
    /// The passed runs with `--before` since the seal, this one included; `None` without `--before`.
    pub comparisons: Option<u32>,
    pub harness: Vec<String>,
    pub head: Head,
    pub before: Option<BeforeJson>,
    pub previous_run: Option<String>,
    pub measures: Vec<Measure>,
    /// The measures whose cost only a person can judge: `change: unknown` (plan, row 66).
    pub needs: Vec<Need>,
    pub flags: Vec<Flag>,
    /// By `selfMs`, the hottest first.
    pub hot: Vec<Hot>,
    pub coverage: Option<CoverageJson>,
    /// `None` without `--before` or when "before" failed.
    pub behaviour: Option<Vec<Behaviour>>,
    /// `None` without the fault repeats.
    pub fault: Option<FaultJson>,
    /// `None` without the second pass, or when the head's failed.
    pub second_pass: Option<SecondPassJson>,
    /// This run's records for `records.json`; a loaded report has none.
    #[serde(skip)]
    pub(crate) records: Arc<Records>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeJson {
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SealJson {
    pub sealed_at: String,
    pub max_tolerance: u16,
    pub journal: Vec<JournalJson>,
}

/// One `<verb>At` per entry, as the session's journal writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, rename_all_fields = "camelCase")]
pub enum JournalJson {
    Sealed {
        sealed_at: String,
        digest: String,
    },
    Resealed {
        resealed_at: String,
        digest: String,
        reason: String,
    },
    Cleaned {
        cleaned_at: String,
    },
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
    /// `None` unless "before" passed: there is no shift to grade.
    pub expect: Option<ExpectJson>,
}

/// The bound that applies, the last acceptance of this head tree over the declared one, and its
/// grade.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpectJson {
    /// `none`, `+X%`, `-X%`, `down` or `unknown`.
    pub change: String,
    /// `None`: no entry names the measure, so it is the implicit `none`. `new-code` for the own hot
    /// time of new code, whatever the entry says.
    pub reason: Option<String>,
    /// `None` for `change: unknown`.
    pub grade: Option<Grade>,
    pub post_hoc: bool,
    /// An acceptance of the measure for another head tree, and none for this one.
    pub stale: bool,
    /// Of the measure in the session, on any head tree.
    pub acceptances: u32,
}

/// With a shift that shows, a grade is never `unmeasurable` (plan, row 66), but for the own hot time
/// of new code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Grade {
    Met,
    Exceeded,
    Surprise,
    Unmeasurable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Need {
    pub measure: String,
    pub reason: String,
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
    /// The run whose repeat made it, when this run took it from its pool; `None` when this run made
    /// it (plan, row 89). The same on `FaultJson` and `SecondPassJson`.
    pub from: Option<String>,
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

/// The fault repeats (plan, row 73): categories of what the head and "before" did under the
/// fault, not times.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaultJson {
    pub from: Option<String>,
    /// `<service>:<port>`.
    pub target: String,
    /// `down` or `slow <delay>ms`.
    pub mode: String,
    pub categories: Categories,
    /// One entry per value of the head other than the one "before" agreed on.
    pub changed: Vec<CategoryChange>,
}

/// Every category on both versions, in one order; `before` is `None` without "before".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Categories {
    pub before: Option<Vec<CategoryJson>>,
    pub head: Vec<CategoryJson>,
}

/// `values`: one per fault repeat in order, `unknown` for one that ended before the fault went on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoryJson {
    pub category: String,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoryChange {
    pub category: String,
    pub before: String,
    pub head: String,
}

/// The second pass (plan, row 74): the workload twice on one state without restore.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecondPassJson {
    pub from: Option<String>,
    /// The files of `expect.unchanged`.
    pub files: Vec<PassFile>,
    /// By version and target.
    pub open: Vec<PassOpen>,
}

/// Whether the file changed from the first pass to the second; `before` is `None` without a
/// passed second pass of "before". `diffs` are the head's, the first pass as `before` and the
/// second as `head`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PassFile {
    pub file: String,
    pub before: Option<PassState>,
    pub head: PassState,
    pub diffs: Vec<Diff>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PassState {
    Same,
    Changed,
}

/// The open connections to `target` after each pass, added up over the instances.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PassOpen {
    pub target: String,
    pub side: String,
    pub first: u64,
    pub second: u64,
}

/// The quiet `--json` line (plan, row 76): `Report::line` or `Line::failed`. `cli` prints it with
/// `next` as command strings, and sets `run` of a failed run that has its folder. The default is
/// the empty line of `--dry-run --json`.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub run: Option<String>,
    #[serde(serialize_with = "slash::option")]
    pub report: Option<Utf8PathBuf>,
    /// As in the report; `None` and 0 without one.
    pub seal: Option<SealJson>,
    pub attempts: u32,
    pub comparisons: Option<u32>,
    /// Only the exceptions.
    pub measures: Vec<LineMeasure>,
    /// Only the files with a diff or a flag that names them; `None` as in the report.
    pub behaviour: Option<Vec<LineFile>>,
    pub fault: Option<LineFault>,
    /// The files of `expect.unchanged` that changed on the head; `None` as in the report.
    pub second_pass: Option<Vec<LinePass>>,
    pub flags: Vec<Flag>,
    pub needs: Vec<Need>,
    pub failure: Option<Failure>,
    /// The measures and files with no exception.
    pub quiet: u32,
    /// Of the quiet ones, the measures graded `unmeasurable` without an `expect` entry (plan, row
    /// 66) or as new code.
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
    pub expect: Option<ExpectJson>,
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

/// `unknown`: the categories nothing compares on.
#[derive(Debug, Clone, Serialize)]
pub struct LineFault {
    pub target: String,
    pub mode: String,
    pub changed: Vec<CategoryChange>,
    pub unknown: Vec<String>,
    pub from: Option<String>,
}

/// `sample`: at most 3 records of the head's diffs; `path`: all of them,
/// `diff/pass2/<file>.json` of the run folder.
#[derive(Debug, Clone, Serialize)]
pub struct LinePass {
    pub file: String,
    pub before: Option<PassState>,
    pub diffs: u32,
    pub sample: Vec<Diff>,
    #[serde(serialize_with = "slash::path")]
    pub path: Utf8PathBuf,
    pub from: Option<String>,
}

/// In a folder of its own: a behaviour file of the same name has `diff/<file>.json`.
pub(crate) fn pass_path(dir: &Utf8Path, file: &str) -> Utf8PathBuf {
    dir.join("diff").join("pass2").join(format!("{file}.json"))
}

impl Report {
    /// `<dir>/diff/<file>.json` with all the diffs of each behaviour file, `<dir>/diff/pass2/`
    /// with those of the second pass, and `<dir>/records.json`, then `<dir>/report.json` through
    /// a temporary file and a rename: a `report.json` is whole and says the rest is.
    pub fn write(&self, dir: &Utf8Path) -> Result<(), ReportError> {
        let behaviour = self.behaviour.iter().flatten();
        let behaviour = behaviour.map(|file| (diff_path(dir, &file.file), &file.diffs));
        let passes = self.second_pass.iter().flat_map(|pass| &pass.files);
        let passes = passes.map(|file| (pass_path(dir, &file.file), &file.diffs));
        for (path, diffs) in behaviour.chain(passes) {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(ReportError::io(parent))?;
            }
            write_json(&path, diffs, false)?;
        }
        write_json(&dir.join(RECORDS), self.records.as_ref(), false)?;
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
    let bytes = bytes.map_err(|error| ReportError::bad(path, error))?;
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
    let bad = |error: serde_json::Error| ReportError::bad(&path, error);
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

pub(crate) fn seal_json(seal: &SealInfo) -> SealJson {
    let journal = seal.journal.iter().map(|entry| match entry {
        Journal::Sealed { sealed_at, digest } => JournalJson::Sealed {
            sealed_at: time(*sealed_at),
            digest: digest.clone(),
        },
        Journal::Resealed {
            resealed_at,
            digest,
            reason,
        } => JournalJson::Resealed {
            resealed_at: time(*resealed_at),
            digest: digest.clone(),
            reason: reason.clone(),
        },
        Journal::Cleaned { cleaned_at } => JournalJson::Cleaned {
            cleaned_at: time(*cleaned_at),
        },
    });
    SealJson {
        sealed_at: time(seal.sealed_at),
        max_tolerance: seal.max_tolerance,
        journal: journal.collect(),
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
