//! The text without `--json` (architecture plan, «Сводка»): one fact per line, with the tokens of
//! the JSON, so a person and the agent read the same words.

use std::fmt;

use camino::Utf8Path;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::flags::Flag;
use crate::raw::Failure;
use crate::schema::{
    IntentJson, Line, LineFault, LineFile, LineMeasure, LinePass, Need, Report, SealJson, slashed,
};

/// The hot functions the summary prints; the report has all of them.
const TOP: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    lines: Vec<String>,
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for line in &self.lines {
            writeln!(f, "{line}")?;
        }
        Ok(())
    }
}

impl Report {
    /// The exceptions and `quiet` are the line's; `cli` prints the path of the report after it.
    pub fn summary(&self) -> Summary {
        let line = self.line(Utf8Path::new(""));
        let mut lines = headlines(&self.flags);
        lines.push(format!("run {}", self.run));
        lines.push(format!(
            "repeat {}  setupTimeout {}  timeout {}",
            self.repeat, self.setup_timeout, self.timeout
        ));
        if !self.harness.is_empty() {
            lines.push(format!("harness {}", self.harness.join(" ")));
        }
        if let Some(before) = &self.before {
            let intent = match (&before.intent, &before.reference) {
                (IntentJson::Ref, Some(name)) => format!("ref {name}"),
                _ => token(&before.intent),
            };
            lines.push(format!(
                "before {intent} {}  takenAt {}",
                before.treeish, before.taken_at
            ));
        }
        let built = |cached: &Option<String>| {
            cached
                .as_ref()
                .map_or_else(|| "built".to_owned(), |run| format!("cached {run}"))
        };
        let mut setup = format!("setup head {}", built(&self.head.setup.cached));
        if let Some(before) = &self.before {
            setup.push_str(&format!("  before {}", built(&before.setup.cached)));
        }
        lines.push(setup);
        if let Some(previous) = &self.previous_run {
            lines.push(format!("previousRun {previous}"));
        }
        lines.extend(seal_text(
            self.seal.as_ref(),
            self.attempts,
            self.comparisons,
        ));
        lines.extend(line.measures.iter().map(measure_text));
        lines.extend(self.needs.iter().map(need_text));
        lines.extend(line.behaviour.iter().flatten().map(file_text));
        lines.extend(line.fault.iter().flat_map(fault_text));
        lines.extend(line.second_pass.iter().flatten().map(pass_text));
        lines.extend(self.flags.iter().map(flag_text));
        for hot in self.hot.iter().take(TOP) {
            let mut text = format!(
                "hot {}ms {}:{} {} {}",
                hot.self_ms, hot.file, hot.line, hot.name, hot.service
            );
            if let Some(before) = &hot.before {
                text.push_str(&format!(" before {}", token(&before.verdict)));
            }
            text.push_str(&format!(" previous {}", token(&hot.previous.verdict)));
            lines.push(text);
        }
        if let Some(coverage) = &self.coverage {
            let mut text = format!(
                "coverage {} called in {}",
                coverage.called.len(),
                coverage.services.join(" ")
            );
            text.push_str(&from_text(coverage.from.as_ref()));
            lines.push(text);
        }
        lines.push(format!("quiet {}", line.quiet));
        Summary { lines }
    }
}

impl Line {
    pub fn summary(&self) -> Summary {
        let mut lines = headlines(&self.flags);
        if let Some(failure) = &self.failure {
            lines.extend(failure_text(failure));
        }
        if let Some(run) = &self.run {
            lines.push(format!("run {run}"));
        }
        lines.extend(seal_text(
            self.seal.as_ref(),
            self.attempts,
            self.comparisons,
        ));
        lines.extend(self.measures.iter().map(measure_text));
        lines.extend(self.needs.iter().map(need_text));
        for file in self.behaviour.iter().flatten() {
            lines.push(format!("{}  {}", file_text(file), slashed(&file.path)));
        }
        lines.extend(self.fault.iter().flat_map(fault_text));
        for file in self.second_pass.iter().flatten() {
            lines.push(format!("{}  {}", pass_text(file), slashed(&file.path)));
        }
        lines.extend(self.flags.iter().map(flag_text));
        lines.push(format!("quiet {}", self.quiet));
        if let Some(report) = &self.report {
            lines.push(format!("report {}", slashed(report)));
        }
        Summary { lines }
    }
}

/// The first lines (plan, rows 23 and 42), in the order of the flags. Without a profiled service
/// there is no coverage to miss: `unprofiled` stays a flag line.
fn headlines(flags: &[Flag]) -> Vec<String> {
    let head = flags.iter().filter_map(|flag| match flag {
        Flag::BeforeFailed { stage } => {
            Some(format!("NO COMPARISON: before failed at {}", token(stage)))
        }
        Flag::CoverageUnknown {
            stage: Some(stage), ..
        } => Some(format!(
            "coverage unknown: repeat failed at {}",
            token(stage)
        )),
        Flag::CoverageUnknown {
            service: Some(service),
            ..
        } => Some(format!("coverage unknown: {service} left no coverage")),
        Flag::CoverageUnknown { .. } => {
            Some("coverage unknown: no service in the coverage repeat".to_owned())
        }
        _ => None,
    });
    head.collect()
}

/// A failed line has no report, so no attempts to print.
fn seal_text(seal: Option<&SealJson>, attempts: u32, comparisons: Option<u32>) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(seal) = seal {
        lines.push(format!(
            "seal {}  maxTolerance {}",
            seal.sealed_at, seal.max_tolerance
        ));
        let journal = seal.journal.iter();
        lines.extend(journal.map(|entry| format!("journal{}", pairs(&fields(entry)))));
    }
    if attempts > 0 {
        let mut text = format!("attempts {attempts}");
        if let Some(comparisons) = comparisons {
            text.push_str(&format!("  comparisons {comparisons}"));
        }
        lines.push(text);
    }
    lines
}

fn failure_text(failure: &Failure) -> Vec<String> {
    let service = failure.service.as_deref().unwrap_or("-");
    let repeat = failure
        .repeat
        .map_or_else(|| "-".to_owned(), |repeat| repeat.to_string());
    let mut lines = vec![format!(
        "failure {} {service} repeat {repeat}: {}",
        token(&failure.phase),
        failure.cause
    )];
    lines.extend(
        failure
            .logs
            .iter()
            .map(|logs| format!("logs {}", slashed(logs))),
    );
    let probes = failure.probes.iter();
    lines.extend(probes.map(|probe| format!("probes {}", slashed(probe))));
    lines
}

fn file_text(file: &LineFile) -> String {
    format!(
        "behaviour {} diffs {}  noiseKnown {}",
        file.file, file.diffs, file.noise_known
    )
}

fn fault_text(fault: &LineFault) -> Vec<String> {
    let from = from_text(fault.from.as_ref());
    let mut lines = vec![format!("fault {} {}{from}", fault.target, fault.mode)];
    for change in &fault.changed {
        lines.push(format!(
            "fault {} before {} head {}",
            change.category, change.before, change.head
        ));
    }
    let unknown = fault.unknown.iter();
    lines.extend(unknown.map(|category| format!("fault {category} unknown")));
    lines
}

fn pass_text(file: &LinePass) -> String {
    format!(
        "secondPass {} before {} diffs {}{}",
        file.file,
        token(&file.before),
        file.diffs,
        from_text(file.from.as_ref())
    )
}

fn from_text(from: Option<&String>) -> String {
    from.map(|run| format!(" from {run}")).unwrap_or_default()
}

fn measure_text(measure: &LineMeasure) -> String {
    let mut text = format!(
        "{} {} {}",
        measure.name,
        measure.median,
        token(&measure.unit)
    );
    if let Some(before) = &measure.before {
        let verdict = token(&before.verdict);
        text.push_str(&format!(" before {} {verdict}", number(before.median)));
    }
    let previous = &measure.previous;
    let verdict = token(&previous.verdict);
    text.push_str(&format!(" previous {} {verdict}", number(previous.median)));
    if let Some(expect) = &measure.expect {
        let grade = token(&expect.grade);
        text.push_str(&format!(" expect {} {grade}", expect.change));
        if expect.post_hoc {
            text.push_str(" postHoc");
        }
        if expect.stale {
            text.push_str(" stale");
        }
        if expect.acceptances > 0 {
            text.push_str(&format!(" acceptances {}", expect.acceptances));
        }
    }
    text
}

fn need_text(need: &Need) -> String {
    format!("needs {}: {}", need.measure, need.reason)
}

/// `<kind>`, then each fact of the flag as `<key> <value>`.
fn flag_text(flag: &Flag) -> String {
    let mut fields = fields(flag);
    let mut text = fields
        .remove("kind")
        .as_ref()
        .map(plain)
        .unwrap_or_default();
    text.push_str(&pairs(&fields));
    text
}

fn fields(value: &impl Serialize) -> Map<String, Value> {
    match serde_json::to_value(value) {
        Ok(Value::Object(fields)) => fields,
        _ => Map::new(),
    }
}

fn pairs(fields: &Map<String, Value>) -> String {
    let pairs = fields.iter();
    pairs
        .map(|(key, value)| format!(" {key} {}", plain(value)))
        .collect()
}

fn number(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| value.to_string())
}

/// The JSON word of a value: `within-noise`, `workload`, `ms`.
fn token(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .map(|value| plain(&value))
        .unwrap_or_default()
}

fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "-".to_owned(),
        other => other.to_string(),
    }
}
