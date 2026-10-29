//! What `make` reads of the run folder (architecture plan, `report`): the top-level `*.jsonl` of
//! each `/stand/out`, the `unchanged` files after each pass of the second pass, the logs of the
//! long-running services up to their cut, and `records.json` of the pooled runs. A broken line of a
//! workload file is a flag, not an error; a result file the stand will not read is `BadResult`.

use std::collections::BTreeMap;
use std::fs::{self, File, Metadata};
use std::io::{self, BufRead, BufReader, Read};
use std::sync::Arc;

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::raw::{PassPair, Raw, Role};
use crate::{Before, ReportError, Side, sides};

pub(crate) const RECORDS: &str = "records.json";

/// The most the stand reads of one result file: it holds the whole file in memory and writes its
/// records into `records.json`.
pub const RESULT_LIMIT: u64 = 64 << 20;

/// A result in `/stand/out` the stand will not read. The guest wrote it, so it is the recipe's
/// fault, not the stand's (batch review 6): `cli` gives it exit 2, whichever reader met it.
#[derive(Debug, thiserror::Error)]
pub enum BadResult {
    #[error(
        "{0}: not a plain file: the stand reads a result in /stand/out only as a plain file, never \
         through a link"
    )]
    NotPlain(Utf8PathBuf),
    #[error(
        "{0}: not a plain folder: the stand reads the results in /stand/out only through plain \
         folders, never through a link"
    )]
    NotPlainFolder(Utf8PathBuf),
    #[error(
        "{path}: {size} bytes, over the {} MiB the stand reads of a result file: write less into it",
        RESULT_LIMIT >> 20
    )]
    TooLarge { path: Utf8PathBuf, size: u64 },
}

/// The rule of a result file, on its `symlink_metadata`: a link there may lead to any file of the
/// host, a FIFO would hold the read forever, and a file over `RESULT_LIMIT` is never read.
pub fn check_result(path: &Utf8Path, metadata: &Metadata) -> Result<(), BadResult> {
    if !metadata.file_type().is_file() {
        return Err(BadResult::NotPlain(path.to_owned()));
    }
    if metadata.len() > RESULT_LIMIT {
        return Err(BadResult::TooLarge {
            path: path.to_owned(),
            size: metadata.len(),
        });
    }
    Ok(())
}

/// The repeat of `FileRecords` that holds the `after-setup` output of setup.
pub(crate) const SETUP: u32 = 0;

/// What `make` takes from the files of a run; only `read_outputs` builds it.
#[derive(Debug, Default)]
pub struct Outputs {
    /// Shared with the `Report` of `make`, which writes it as `records.json`.
    pub(crate) records: Arc<Records>,
    /// By the log of an instance.
    errors: BTreeMap<Utf8PathBuf, u32>,
    /// The first broken line of each workload file, from 1.
    pub(crate) bad_lines: BTreeMap<String, u32>,
    /// `records.json` of each pooled run in the order of `Context.pool`; `None` when it does not
    /// read. Empty unless "before" passed or the second pass ran: only their masks read it.
    pub(crate) pool: Vec<Option<Records>>,
    /// The records of each `unchanged` file after the first and the second pass, by file and
    /// version; a version whose second pass failed has none.
    pub(crate) passes: BTreeMap<String, BTreeMap<Side, [Vec<Value>; 2]>>,
}

impl Outputs {
    /// Error lines and Node warnings of the log before its cut.
    pub(crate) fn errors(&self, log: &Utf8Path) -> u32 {
        self.errors.get(log).copied().unwrap_or(0)
    }
}

/// The behaviour records of a run by file, version and repeat, after `{"line": ...}` and before
/// `ignore`: this is `records.json`, from which the later runs of the pool learn their noise mask.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Records {
    pub(crate) files: BTreeMap<String, FileRecords>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct FileRecords {
    #[serde(default)]
    pub(crate) head: BTreeMap<u32, Vec<Value>>,
    #[serde(default)]
    pub(crate) before: BTreeMap<u32, Vec<Value>>,
}

impl FileRecords {
    pub(crate) fn side(&self, side: Side) -> &BTreeMap<u32, Vec<Value>> {
        match side {
            Side::Head => &self.head,
            Side::Before => &self.before,
        }
    }

    fn side_mut(&mut self, side: Side) -> &mut BTreeMap<u32, Vec<Value>> {
        match side {
            Side::Head => &mut self.head,
            Side::Before => &mut self.before,
        }
    }
}

/// `pool`: the folders of the pooled runs, in the order of `Context.pool`; their `records.json`
/// feeds the noise mask.
pub fn read_outputs(raw: &Raw, pool: &[Utf8PathBuf]) -> Result<Outputs, ReportError> {
    let mut outputs = Outputs::default();
    for (side, setup, repeats) in sides(raw) {
        outputs.files(raw, side, SETUP, &setup.out_dir)?;
        for repeat in repeats {
            outputs.files(raw, side, repeat.repeat, &repeat.out_dir)?;
            let instances = repeat.instances.iter();
            for instance in instances.filter(|instance| instance.role == Role::LongRunning) {
                let count = error_lines(&instance.log, instance.log_cut)?;
                outputs.errors.insert(instance.log.clone(), count);
            }
        }
    }
    // A pooled run whose records do not read teaches the mask nothing, as an unreadable pooled
    // report is left out of the pool. The second pass masks by the measured repeats of its
    // version, with or without "before" (batch review 6).
    if matches!(Before::of(raw), Before::Passed(_)) || raw.second_pass.is_some() {
        let pooled = pool.iter().map(|dir| read_records(&dir.join(RECORDS)));
        outputs.pool = pooled.collect();
    }
    if let Some(outcome) = &raw.second_pass {
        for (side, pair) in outcome.sides() {
            let Some(PassPair::Passed { first, second }) = pair else {
                continue;
            };
            for file in &outcome.unchanged {
                let probe = raw.probes.contains_key(file);
                let texts = [
                    pass_file(&first.out_dir, file)?,
                    pass_file(&second.out_dir, file)?,
                ];
                let records = texts.map(|text| parse(&text, probe).0);
                let passes = outputs.passes.entry(file.clone()).or_default();
                passes.insert(side, records);
            }
        }
    }
    Ok(outputs)
}

/// `file` of `expect.unchanged` under `dir`, a path with `/`: every folder on the way a plain
/// folder, and the file under the rule of a top-level one. A pass that wrote no such file has no
/// records of it.
fn pass_file(dir: &Utf8Path, file: &str) -> Result<String, ReportError> {
    let mut path = dir.to_owned();
    let parts: Vec<&str> = file.split('/').collect();
    for (index, part) in parts.iter().enumerate() {
        path.push(part);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(String::new()),
            Err(error) => return Err(ReportError::io(&path)(error)),
        };
        if index + 1 < parts.len() && !metadata.file_type().is_dir() {
            return Err(BadResult::NotPlainFolder(path).into());
        }
    }
    readable(&path)?;
    let bytes = fs::read(&path).map_err(ReportError::io(&path))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

impl Outputs {
    /// The top-level `*.jsonl` of one `/stand/out`: not the `.err` and `.status` beside a probe's
    /// output, and not a name that is not UTF-8, which no JSON could name.
    fn files(
        &mut self,
        raw: &Raw,
        side: Side,
        repeat: u32,
        dir: &Utf8Path,
    ) -> Result<(), ReportError> {
        for entry in fs::read_dir(dir).map_err(ReportError::io(dir))? {
            let entry = entry.map_err(ReportError::io(dir))?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if !name.ends_with(".jsonl") {
                continue;
            }
            let path = dir.join(&name);
            readable(&path)?;
            let bytes = fs::read(&path).map_err(ReportError::io(&path))?;
            let probe = raw.probes.contains_key(&name);
            let (records, bad) = parse(&String::from_utf8_lossy(&bytes), probe);
            if let Some(line) = bad {
                self.bad_lines.entry(name.clone()).or_insert(line);
            }
            let files = &mut Arc::make_mut(&mut self.records).files;
            let file = files.entry(name).or_default();
            file.side_mut(side).insert(repeat, records);
        }
        Ok(())
    }
}

/// Every VM is gone by now, so nothing swaps or grows the file after this look.
fn readable(path: &Utf8Path) -> Result<(), ReportError> {
    let metadata = fs::symlink_metadata(path).map_err(ReportError::io(path))?;
    Ok(check_result(path, &metadata)?)
}

/// A probe's line that is not JSON is `{"line": ...}` (plan, row 62); a broken line of a workload
/// file is left out, and the first one comes back. Blank lines are no records.
fn parse(text: &str, probe: bool) -> (Vec<Value>, Option<u32>) {
    let mut records = Vec::new();
    let mut bad = None;
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(record) => records.push(record),
            Err(_) if probe => records.push(json!({ "line": line })),
            Err(_) => bad = bad.or(Some(u32::try_from(index + 1).unwrap_or(u32::MAX))),
        }
    }
    (records, bad)
}

fn read_records(path: &Utf8Path) -> Option<Records> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// The lines of the log before `cut`: what the service logged after the workload ended is not the
/// measured window.
fn error_lines(log: &Utf8Path, cut: u64) -> Result<u32, ReportError> {
    let file = File::open(log).map_err(ReportError::io(log))?;
    // Line by line: a service may log gigabytes in the window.
    let mut count: u32 = 0;
    for line in BufReader::new(file.take(cut)).split(b'\n') {
        let line = line.map_err(ReportError::io(log))?;
        let line = line.strip_suffix(b"\r").unwrap_or(&line);
        if is_error(&String::from_utf8_lossy(line)) {
            count = count.saturating_add(1);
        }
    }
    Ok(count)
}

/// The plan's rule (Optional Assumptions, «Строка уровня error»): a JSON line by its `level` from
/// 50, or `level` or `severity` `error` or `fatal`; any other line by the word `ERROR` or `FATAL` or
/// a start `Error:`; a Node warning, `(node:<pid>) <Name>Warning:` or `[DEP<n>]`, counts the same.
fn is_error(line: &str) -> bool {
    let line = line.trim();
    if line.starts_with('{')
        && let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(line)
    {
        return json_error(&fields);
    }
    word(line, "ERROR") || word(line, "FATAL") || line.starts_with("Error:") || node_warning(line)
}

fn json_error(fields: &Map<String, Value>) -> bool {
    let named = |value: &Value| {
        value.as_str().is_some_and(|text| {
            text.eq_ignore_ascii_case("error") || text.eq_ignore_ascii_case("fatal")
        })
    };
    let level = fields.get("level");
    level
        .and_then(Value::as_f64)
        .is_some_and(|level| level >= 50.0)
        || level.is_some_and(named)
        || fields.get("severity").is_some_and(named)
}

/// `token` with no letter, digit or `_` on either side: `[ERROR]` and `ERROR:`, not `ERRORS`.
fn word(line: &str, token: &str) -> bool {
    let inner = |c: char| c.is_alphanumeric() || c == '_';
    line.match_indices(token).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + token.len()..].chars().next();
        !before.is_some_and(inner) && !after.is_some_and(inner)
    })
}

fn node_warning(line: &str) -> bool {
    let named = line
        .strip_prefix("(node:")
        .and_then(|rest| rest.split_once(") "))
        .filter(|(pid, _)| !pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit()))
        .is_some_and(|(_, rest)| {
            let name = rest.split(' ').next();
            name.is_some_and(|name| name.ends_with("Warning:"))
        });
    named
        || line.match_indices("[DEP").any(|(at, _)| {
            let rest = &line[at + "[DEP".len()..];
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            digits > 0 && rest[digits..].starts_with(']')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_lines_follow_the_rule_of_the_plan() {
        let errors = [
            r#"{"level":50,"msg":"boom"}"#,
            r#"{"level":"error","msg":"boom"}"#,
            r#"{"severity":"FATAL"}"#,
            "2026-10-22 10:00:00 UTC [42] ERROR:  relation \"orders\" does not exist",
            "[FATAL] out of memory",
            "Error: connect ECONNREFUSED 10.0.0.2:5432",
            "(node:17) MaxListenersExceededWarning: Possible EventEmitter memory leak detected.",
            "(node:17) [DEP0005] DeprecationWarning: Buffer() is deprecated",
            "(node:17) TimeoutOverflowWarning: 4294967296 does not fit into a 32-bit signed integer.",
        ];
        let others = [
            r#"{"level":30,"msg":"ERROR in a message of an info line"}"#,
            "no ERRORS today",
            "the TypeError: was caught",
            "(node:17) ExperimentalWarning is a word, not a warning line",
            "[DEPRECATED] not a code",
            "",
        ];
        for line in errors {
            assert!(is_error(line), "{line}");
        }
        for line in others {
            assert!(!is_error(line), "{line}");
        }
    }

    #[test]
    fn a_probe_line_that_is_not_json_is_a_line_record_and_a_workload_line_is_bad() {
        let text = "{\"id\":1}\n\nnot json\n{\"id\":2}\n";
        let (records, bad) = parse(text, true);
        let expected = [
            json!({"id": 1}),
            json!({"line": "not json"}),
            json!({"id": 2}),
        ];
        assert_eq!(records, expected);
        assert_eq!(bad, None);
        let (records, bad) = parse(text, false);
        assert_eq!(records, [json!({"id": 1}), json!({"id": 2})]);
        assert_eq!(bad, Some(3));
    }
}
