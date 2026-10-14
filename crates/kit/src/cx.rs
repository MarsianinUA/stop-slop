use std::fmt::Display;

use oxc_span::Span;

use crate::{Facts, File};

pub struct Cx<'s, 'a> {
    file: &'s File<'a>,
    facts: &'s dyn Facts,
    out: &'s mut Vec<Report>,
}

impl<'s, 'a> Cx<'s, 'a> {
    pub fn new(file: &'s File<'a>, facts: &'s dyn Facts, out: &'s mut Vec<Report>) -> Self {
        Cx { file, facts, out }
    }

    pub fn file(&self) -> &'s File<'a> {
        self.file
    }

    pub fn facts(&self) -> &'s dyn Facts {
        self.facts
    }

    /// Records the finding at once: `cx.report(span);` alone is a finding,
    /// `.arg` and `.fix` only add to it.
    pub fn report(&mut self, span: Span) -> Reported<'_> {
        Reported {
            report: self.out.push_mut(Report {
                span,
                args: Vec::new(),
                edits: Vec::new(),
            }),
        }
    }
}

#[derive(Debug)]
pub struct Reported<'r> {
    report: &'r mut Report,
}

impl Reported<'_> {
    /// Fills `{key}` in the rule's message.
    pub fn arg(self, key: &'static str, value: impl Display) -> Self {
        self.report.args.push((key, value.to_string()));
        self
    }

    pub fn fix(self, edits: impl IntoIterator<Item = Edit>) {
        self.report.edits.extend(edits);
    }
}

#[derive(Debug)]
pub struct Edit {
    /// An empty span inserts `text`.
    pub span: Span,
    pub text: String,
}

#[derive(Debug)]
pub struct Report {
    pub span: Span,
    pub args: Vec<(&'static str, String)>,
    pub edits: Vec<Edit>,
}
