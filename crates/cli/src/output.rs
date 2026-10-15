use std::io::{self, Write as _};

use engine::RuleState;
use kit::serde::Serialize;
use kit::{Files, RuleKind, Ruleset, Source};

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

/// One line: the reader is an agent, and indentation costs it tokens.
pub(crate) fn rules_json(states: &[RuleState]) -> String {
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
    let mut json = serde_json::to_string(&rules).expect("a list of strings serializes");
    json.push('\n');
    json
}

/// A closed pipe (`| head`) is not a failure of the command: the exit code stays as it was.
pub(crate) fn print(text: &str) {
    let _ = io::stdout().lock().write_all(text.as_bytes());
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
