use std::fmt::{Display, Write as _};
use std::io::{self, Write as _};

use engine::{Finding, RuleState};
use kit::serde::Serialize;
use kit::{Files, FindingKind, RuleKind, Ruleset, Source};
use repo::RepoError;
use scope::{Base, BaseReason, SkipReason};
use state::IgnoreEntry;

use crate::check::Checked;
use crate::error::CliError;
use crate::hot::{Coverage, Found};

/// As V8 names a function without a name.
const ANONYMOUS: &str = "(anonymous)";

#[derive(Clone, Copy)]
pub(crate) enum JsonLayout {
    /// The default: the reader is an agent, and indentation costs it tokens.
    Line,
    /// `--pretty`, for a person reading a saved file.
    Pretty,
}

impl JsonLayout {
    pub(crate) fn new(pretty: bool) -> Self {
        if pretty { Self::Pretty } else { Self::Line }
    }
}

#[derive(Serialize)]
#[serde(crate = "kit::serde", rename_all = "camelCase")]
struct RuleJson {
    id: &'static str,
    group: &'static str,
    description: &'static str,
    kind: &'static str,
    fix: bool,
    files: &'static str,
    source: Vec<&'static str>,
    message: &'static str,
    agent: Option<&'static str>,
    active: bool,
}

#[derive(Serialize)]
#[serde(crate = "kit::serde", rename_all = "camelCase")]
struct CheckJson<'c> {
    pr: Option<PrJson<'c>>,
    base: Option<BaseJson<'c>>,
    files: Vec<&'c str>,
    skipped: Vec<SkippedJson<'c>>,
    findings: Vec<FindingJson<'c>>,
    fixed: Vec<FixedJson<'c>>,
}

#[derive(Serialize)]
#[serde(crate = "kit::serde", rename_all = "camelCase")]
struct PrJson<'c> {
    number: u32,
    head: &'c str,
    base: &'c str,
    title: &'c str,
}

#[derive(Serialize)]
#[serde(crate = "kit::serde", rename_all = "camelCase")]
struct BaseJson<'c> {
    #[serde(rename = "ref")]
    reference: &'c str,
    sha: String,
    reason: &'static str,
}

#[derive(Serialize)]
#[serde(crate = "kit::serde", rename_all = "camelCase")]
struct SkippedJson<'c> {
    file: &'c str,
    reason: &'static str,
}

#[derive(Serialize)]
#[serde(crate = "kit::serde", rename_all = "camelCase")]
struct FindingJson<'c> {
    rule: &'static str,
    kind: &'static str,
    file: &'c str,
    line: u32,
    column: u32,
    end_line: u32,
    end_column: u32,
    message: &'c str,
    in_hunk: Option<bool>,
    fixable: bool,
    fingerprint: String,
}

#[derive(Serialize)]
#[serde(crate = "kit::serde", rename_all = "camelCase")]
struct FixedJson<'c> {
    rule: &'static str,
    file: &'c str,
    count: u32,
}

#[derive(Serialize)]
#[serde(crate = "kit::serde", rename_all = "camelCase")]
struct HotJson<'c> {
    base: Option<BaseJson<'c>>,
    hot: Vec<HotRowJson<'c>>,
    unexercised: Option<Vec<UnexercisedJson<'c>>>,
    coverage: &'static str,
}

#[derive(Serialize)]
#[serde(crate = "kit::serde", rename_all = "camelCase")]
struct HotRowJson<'c> {
    service: &'c str,
    file: &'c str,
    function: Option<&'c str>,
    line: u32,
    end_line: u32,
    self_ms: f64,
    total_ms: f64,
    entries: Option<u64>,
    stale: bool,
    findings: Vec<FindingJson<'c>>,
}

#[derive(Serialize)]
#[serde(crate = "kit::serde", rename_all = "camelCase")]
struct UnexercisedJson<'c> {
    file: &'c str,
    function: Option<&'c str>,
    line: u32,
    end_line: u32,
    stale: bool,
}

#[derive(Serialize)]
#[serde(crate = "kit::serde", rename_all = "camelCase")]
struct FailureLine<'f> {
    failure: FailureJson<'f>,
    next: &'f [String],
}

#[derive(Serialize)]
#[serde(crate = "kit::serde", rename_all = "camelCase")]
struct FailureJson<'f> {
    phase: &'static str,
    cause: &'f str,
}

pub(crate) fn rules_text(set: &Ruleset) -> String {
    let width = set
        .rules
        .iter()
        .map(|def| def.meta.id.len())
        .max()
        .unwrap_or(0);
    set.rules
        .iter()
        .map(|def| {
            let meta = &def.meta;
            let fix = if meta.fix { "fix" } else { "-" };
            let kind = rule_kind(meta.kind);
            format!(
                "{:width$}  {kind:9}  {fix:3}  {}\n",
                meta.id, meta.description
            )
        })
        .collect()
}

pub(crate) fn rules_json(states: &[RuleState], layout: JsonLayout) -> String {
    let rules: Vec<RuleJson> = states
        .iter()
        .map(|state| {
            let meta = state.meta;
            RuleJson {
                id: meta.id,
                group: meta.group,
                description: meta.description,
                kind: rule_kind(meta.kind),
                fix: meta.fix,
                files: files(meta.files),
                source: meta.source.iter().copied().map(source).collect(),
                message: meta.message,
                agent: meta.agent,
                active: state.active,
            }
        })
        .collect();
    json_text(&rules, layout)
}

pub(crate) fn check_json(checked: &Checked, layout: JsonLayout) -> String {
    let report = CheckJson {
        pr: checked.pr.as_ref().map(|pr| PrJson {
            number: pr.number,
            head: &pr.head,
            base: &pr.base,
            title: &pr.title,
        }),
        base: checked.base.as_ref().map(base_json),
        files: checked.files.iter().map(|path| path.as_str()).collect(),
        skipped: checked
            .skipped
            .iter()
            .map(|skipped| SkippedJson {
                file: skipped.path.as_str(),
                reason: skip_reason(skipped.reason),
            })
            .collect(),
        findings: checked.findings.iter().map(finding_json).collect(),
        fixed: checked
            .fixed
            .iter()
            .map(|fixed| FixedJson {
                rule: fixed.rule,
                file: fixed.file.as_str(),
                count: fixed.count,
            })
            .collect(),
    };
    json_text(&report, layout)
}

pub(crate) fn check_text(checked: &Checked) -> String {
    let mut text = String::new();
    // `write!` to a `String` does not fail.
    if let Some(pr) = &checked.pr {
        let _ = writeln!(text, "pr {} {} into {}", pr.number, sha7(&pr.head), pr.base);
    }
    if let Some(base) = &checked.base {
        base_line(&mut text, base);
    }
    for skipped in &checked.skipped {
        let reason = skip_reason(skipped.reason);
        let _ = writeln!(text, "skipped {} ({reason})", skipped.path);
    }
    for fixed in &checked.fixed {
        let _ = writeln!(text, "fixed {} {} x{}", fixed.file, fixed.rule, fixed.count);
    }
    for finding in &checked.findings {
        finding_line(&mut text, finding);
    }
    text
}

pub(crate) fn hot_json(base: Option<&Base>, found: &Found<'_>, layout: JsonLayout) -> String {
    let report = HotJson {
        base: base.map(base_json),
        hot: found
            .hot
            .iter()
            .map(|row| HotRowJson {
                service: &row.service,
                file: row.file.as_str(),
                function: row.function.as_deref(),
                line: row.line,
                end_line: row.end_line,
                self_ms: row.self_ms,
                total_ms: row.total_ms,
                entries: row.entries,
                stale: row.stale,
                findings: row.findings.iter().copied().map(finding_json).collect(),
            })
            .collect(),
        unexercised: match &found.coverage {
            Coverage::Collected(functions) => Some(
                functions
                    .iter()
                    .map(|function| UnexercisedJson {
                        file: function.file.as_str(),
                        function: function.function.as_deref(),
                        line: function.line,
                        end_line: function.end_line,
                        stale: function.stale,
                    })
                    .collect(),
            ),
            Coverage::NotCollected | Coverage::Unknown(_) => None,
        },
        coverage: match found.coverage {
            Coverage::Collected(_) => "collected",
            Coverage::NotCollected => "not-collected",
            Coverage::Unknown(_) => "unknown",
        },
    };
    json_text(&report, layout)
}

pub(crate) fn hot_text(base: Option<&Base>, found: &Found<'_>) -> String {
    let mut text = String::new();
    if let Some(base) = base {
        base_line(&mut text, base);
    }
    for row in &found.hot {
        let function = row.function.as_deref().unwrap_or(ANONYMOUS);
        let mut facts = counted(row.findings.len() as u64, "finding", "findings");
        if let Some(entries) = row.entries {
            facts.push_str(", ");
            facts.push_str(&counted(entries, "entry", "entries"));
        }
        let stale = if row.stale { " stale" } else { "" };
        // The order of the stand's summary; the service tells apart two rows of one function.
        let (ms, file, line, service) = (row.self_ms, row.file, row.line, &row.service);
        let _ = writeln!(
            text,
            "{ms}ms {file}:{line} {function} {service} ({facts}){stale}"
        );
    }
    match &found.coverage {
        Coverage::Collected(functions) => {
            for function in functions {
                let name = function.function.as_deref().unwrap_or(ANONYMOUS);
                let stale = if function.stale { " stale" } else { "" };
                let (file, line) = (function.file, function.line);
                let _ = writeln!(text, "unexercised {file}:{line} {name}{stale}");
            }
        }
        Coverage::NotCollected if found.files => text.push_str("coverage: not collected\n"),
        Coverage::Unknown(reasons) if found.files => {
            let _ = writeln!(text, "coverage: unknown: {}", reasons.join("; "));
        }
        Coverage::NotCollected | Coverage::Unknown(_) => {}
    }
    text
}

fn counted(count: u64, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

/// Over 20 findings, 19 and a line with the rest: the agent sees the list is cut, and its next
/// edit shows the others.
pub(crate) fn hook_text(findings: &[Finding]) -> String {
    const MAX_LINES: usize = 20;
    let shown = if findings.len() > MAX_LINES {
        MAX_LINES - 1
    } else {
        findings.len()
    };
    let mut text = String::new();
    for finding in &findings[..shown] {
        finding_line(&mut text, finding);
    }
    if shown < findings.len() {
        let _ = writeln!(text, "stop-slop: {} more errors", findings.len() - shown);
    }
    text
}

pub(crate) fn ignore_text(entries: &[IgnoreEntry]) -> String {
    let mut text = String::new();
    for entry in entries {
        let _ = writeln!(text, "ignored {} {}", entry.fingerprint, entry.file);
    }
    text
}

pub(crate) fn failure_text(cause: &str, next: &[String]) -> String {
    let mut text = format!("stop-slop: {cause}\n");
    for command in next {
        let _ = writeln!(text, "next: {command}");
    }
    text
}

/// What `--json` prints on 2 and 3 instead of the command's JSON. The stderr of git in `cause`
/// can span lines, and the JSON keeps it on one.
pub(crate) fn failure_json(phase: &'static str, cause: &str, next: &[String]) -> String {
    let line = FailureLine {
        failure: FailureJson {
            phase,
            cause: &kit::one_line(cause),
        },
        next,
    };
    json_text(&line, JsonLayout::Line)
}

/// A closed pipe (`| head`) is the reader's choice and keeps the exit code. Any other error,
/// such as a full disk, cuts the output short, so the command fails rather than exit 0 or 1
/// with half a JSON.
pub(crate) fn print(text: &str) -> Result<(), CliError> {
    let mut stdout = io::stdout().lock();
    match stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Err(source) if source.kind() != io::ErrorKind::BrokenPipe => Err(RepoError::Io {
            path: "stdout".into(),
            source,
        }
        .into()),
        _ => Ok(()),
    }
}

/// An error writing stderr has nowhere to be reported.
pub(crate) fn print_err(text: &str) {
    let _ = io::stderr().lock().write_all(text.as_bytes());
}

fn base_json(base: &Base) -> BaseJson<'_> {
    BaseJson {
        reference: &base.reference,
        sha: sha7(&base.oid),
        reason: base_reason(base.reason),
    }
}

/// The one form of a finding in `check --json` and `hot --json`.
fn finding_json(finding: &Finding) -> FindingJson<'_> {
    FindingJson {
        rule: finding.rule,
        kind: finding_kind(finding.kind),
        file: finding.file.as_str(),
        line: finding.start.line,
        column: finding.start.column,
        end_line: finding.end.line,
        end_column: finding.end.column,
        message: &finding.message,
        in_hunk: finding.in_hunk,
        fixable: finding.fixable,
        fingerprint: finding.fingerprint.to_string(),
    }
}

fn base_line(text: &mut String, base: &Base) {
    let (sha, reason) = (sha7(&base.oid), base_reason(base.reason));
    let _ = writeln!(text, "base {} {sha} ({reason})", base.reference);
}

fn finding_line(text: &mut String, finding: &Finding) {
    let start = &finding.start;
    let _ = writeln!(
        text,
        "{}:{}:{} {} {}",
        finding.file, start.line, start.column, finding.rule, finding.message
    );
}

fn json_text(value: &impl Serialize, layout: JsonLayout) -> String {
    let json = match layout {
        JsonLayout::Line => serde_json::to_string(value),
        JsonLayout::Pretty => serde_json::to_string_pretty(value),
    };
    let mut json = json.expect("strings, numbers and bools serialize");
    json.push('\n');
    json
}

fn sha7(oid: impl Display) -> String {
    let mut sha = oid.to_string();
    sha.truncate(7);
    sha
}

fn rule_kind(kind: RuleKind) -> &'static str {
    match kind {
        RuleKind::Error => "error",
        RuleKind::Candidate => "candidate",
        RuleKind::Agent => "agent",
    }
}

fn files(files: Files) -> &'static str {
    match files {
        Files::All => "all",
        Files::Code => "code",
        Files::Tests => "tests",
    }
}

fn source(source: Source) -> &'static str {
    match source {
        Source::Personal => "personal",
        Source::Direct => "direct",
        Source::Project => "project",
        Source::AntiSlop => "anti-slop",
    }
}

fn finding_kind(kind: FindingKind) -> &'static str {
    match kind {
        FindingKind::Error => "error",
        FindingKind::Candidate => "candidate",
        FindingKind::ParseError => "parse-error",
    }
}

fn base_reason(reason: BaseReason) -> &'static str {
    match reason {
        BaseReason::Flag => "flag",
        BaseReason::Pr => "pr",
        BaseReason::Ancestor => "ancestor",
        BaseReason::Default => "default",
    }
}

fn skip_reason(reason: SkipReason) -> &'static str {
    match reason {
        SkipReason::AlreadyInBase => "already-in-base",
    }
}
