//! The only reads of the report (architecture plan, `report`): the logs of the long-running
//! services up to their cut.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{Map, Value};

use crate::raw::{Raw, Role};
use crate::{ReportError, sides};

/// What `make` takes from the files of a run; only `read_outputs` builds it.
#[derive(Debug, Default)]
pub struct Outputs {
    /// By the log of an instance.
    errors: BTreeMap<Utf8PathBuf, u32>,
}

impl Outputs {
    /// Error lines and Node warnings of the log before its cut.
    pub(crate) fn errors(&self, log: &Utf8Path) -> u32 {
        self.errors.get(log).copied().unwrap_or(0)
    }
}

/// `pool`: the folders of the pooled runs, in the order of `Context.pool`.
pub fn read_outputs(raw: &Raw, _pool: &[Utf8PathBuf]) -> Result<Outputs, ReportError> {
    let mut outputs = Outputs::default();
    for (_, _, repeats) in sides(raw) {
        for repeat in repeats {
            let instances = repeat.instances.iter();
            for instance in instances.filter(|instance| instance.role == Role::LongRunning) {
                let count = error_lines(&instance.log, instance.log_cut)?;
                outputs.errors.insert(instance.log.clone(), count);
            }
        }
    }
    Ok(outputs)
}

/// The lines of the log before `cut`: what the service logged after the workload ended is not the
/// measured window.
fn error_lines(log: &Utf8Path, cut: u64) -> Result<u32, ReportError> {
    let mut bytes = Vec::new();
    File::open(log)
        .and_then(|file| file.take(cut).read_to_end(&mut bytes))
        .map_err(ReportError::io(log))?;
    let text = String::from_utf8_lossy(&bytes);
    let count = text.lines().filter(|line| is_error(line)).count();
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
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
}
