use std::fmt::Write as _;
use std::io::{self, Write as _};

use engine::{Finding, RuleState};
use kit::serde::Serialize;
use kit::{Files, FindingKind, RuleKind, Ruleset, Source};
use repo::RepoError;
use scope::{Base, BaseReason, SkipReason};
use state::IgnoreEntry;

use crate::check::Checked;
use crate::error::CliError;

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
    base: Option<BaseJson<'c>>,
    files: Vec<&'c str>,
    skipped: Vec<SkippedJson<'c>>,
    findings: Vec<FindingJson<'c>>,
    fixed: Vec<FixedJson<'c>>,
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
        base: checked.base.as_ref().map(|base| BaseJson {
            reference: &base.reference,
            sha: sha7(base),
            reason: base_reason(base.reason),
        }),
        files: checked.files.iter().map(|path| path.as_str()).collect(),
        skipped: checked
            .skipped
            .iter()
            .map(|skipped| SkippedJson {
                file: skipped.path.as_str(),
                reason: skip_reason(skipped.reason),
            })
            .collect(),
        findings: checked
            .findings
            .iter()
            .map(|finding| FindingJson {
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
            })
            .collect(),
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
    if let Some(base) = &checked.base {
        let reason = base_reason(base.reason);
        let _ = writeln!(text, "base {} {} ({reason})", base.reference, sha7(base));
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

fn sha7(base: &Base) -> String {
    let mut sha = base.oid.to_string();
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
