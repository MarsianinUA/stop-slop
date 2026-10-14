use std::borrow::Cow;
use std::collections::BTreeSet;

use camino::{Utf8Path, Utf8PathBuf};
use kit::oxc::ast::ast::Expression;
use kit::prelude::{AstKind, Span};
use kit::{FindingKind, Position};

use crate::{Finding, Fingerprint, LineRange};

pub(crate) const PARSE_ERROR: &str = "parse-error";

/// A finding before the parts that depend on its file: hunk, fingerprint, ignore list.
pub(crate) struct Draft<'t> {
    pub rule: &'static str,
    pub kind: FindingKind,
    pub message: String,
    pub start: Position,
    pub end: Position,
    pub fixable: bool,
    /// The source text of the finding.
    pub place: &'t str,
    /// The named function the finding starts in, or `""`.
    pub function: &'t str,
}

pub(crate) struct FileFindings<'f> {
    pub path: &'f Utf8Path,
    pub changed: Option<&'f [LineRange]>,
    pub ignored: &'f BTreeSet<(Utf8PathBuf, String)>,
    pub findings: Vec<Finding>,
}

impl FileFindings<'_> {
    /// Drops the draft when its (file, fingerprint) pair is ignored.
    pub fn push(&mut self, draft: Draft) {
        let key = (
            self.path.to_owned(),
            fingerprint(draft.rule, draft.place, draft.function),
        );
        if self.ignored.contains(&key) {
            return;
        }
        let line = draft.start.line;
        self.findings.push(Finding {
            rule: draft.rule,
            kind: draft.kind,
            file: key.0,
            start: draft.start,
            end: draft.end,
            message: draft.message,
            in_hunk: self.changed.map(|ranges| {
                ranges
                    .iter()
                    .any(|range| (range.start..=range.end).contains(&line))
            }),
            fixable: draft.fixable,
            fingerprint: Fingerprint(key.1),
        });
    }
}

/// No line number goes in, so the fingerprint survives lines added above.
fn fingerprint(rule: &str, place: &str, function: &str) -> String {
    // FNV-1a 64: `std`'s hashers may change between Rust versions.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in place.bytes().chain([0]).chain(function.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{rule}:{:06x}", hash >> 40)
}

/// Replaces each `{key}` of the message with its value in one pass, so a value that holds
/// `{other}` (source text often does) stays as it is. Braces without a known key stay too.
pub(crate) fn fill(message: &str, args: &[(&'static str, String)]) -> String {
    let mut filled = String::with_capacity(message.len());
    let mut rest = message;
    while let Some(open) = rest.find('{') {
        filled.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let arg = after.find('}').and_then(|close| {
            let (_, value) = args.iter().find(|(key, _)| *key == &after[..close])?;
            Some((close, value))
        });
        match arg {
            Some((close, value)) => {
                filled.push_str(value);
                rest = &after[close + 1..];
            }
            None => {
                filled.push('{');
                rest = after;
            }
        }
    }
    filled.push_str(rest);
    filled
}

/// The name for findings inside `kind`: a named function, or a method, class field, object
/// property or variable holding a function. Anonymous callbacks get none, so their findings
/// take the name of the function outside.
pub(crate) fn function_name<'a>(kind: AstKind<'a>) -> Option<Cow<'a, str>> {
    match kind {
        AstKind::Function(function) => function.name().map(|name| Cow::Borrowed(name.as_str())),
        AstKind::MethodDefinition(method) => method.key.name(),
        AstKind::PropertyDefinition(field)
            if field.value.as_ref().is_some_and(Expression::is_function) =>
        {
            field.key.name()
        }
        AstKind::ObjectProperty(property) if property.value.is_function() => property.key.name(),
        AstKind::VariableDeclarator(declarator)
            if declarator
                .init
                .as_ref()
                .is_some_and(Expression::is_function) =>
        {
            declarator
                .id
                .get_identifier_name()
                .map(|name| Cow::Borrowed(name.as_str()))
        }
        _ => None,
    }
}

/// The innermost of `functions` that `offset` falls in.
pub(crate) fn function_around<'f>(functions: &'f [(Span, Cow<'_, str>)], offset: u32) -> &'f str {
    functions
        .iter()
        .filter(|(span, _)| span.start <= offset && offset < span.end)
        .min_by_key(|(span, _)| span.size())
        .map_or("", |(_, name)| name)
}

pub(crate) fn sort(findings: &mut [Finding]) {
    fn key(f: &Finding) -> (&Utf8Path, u32, u32, &str) {
        (&f.file, f.start.line, f.start.column, f.rule)
    }
    findings.sort_by(|a, b| key(a).cmp(&key(b)));
}
