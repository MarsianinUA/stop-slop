use kit::oxc::allocator::Allocator;
use kit::oxc::parser::{ParseOptions, Parser};
use kit::oxc::semantic::SemanticBuilder;
use kit::oxc::span::{GetSpan, SourceType, Span};
use kit::{Cx, Facts, File, FindingKind, LineIndex, Position, Report, RuleKind};
use rayon::prelude::*;

use crate::finding::{self, Draft, FileFindings, PARSE_ERROR};
use crate::{Engine, Finding, Mode, RunReport, SourceFile, text};

impl Engine {
    /// The findings are sorted, so the order does not depend on the number of threads.
    pub fn run(&self, files: Vec<SourceFile>, facts: &dyn Facts, mode: Mode) -> RunReport {
        let mut findings: Vec<Finding> = files
            .into_par_iter()
            .map_init(Allocator::default, |allocator, file| {
                allocator.reset();
                self.check_file(allocator, &file, facts, mode)
            })
            .flatten_iter()
            .collect();
        finding::sort(&mut findings);
        RunReport { findings }
    }

    fn check_file(
        &self,
        allocator: &Allocator,
        file: &SourceFile,
        facts: &dyn Facts,
        mode: Mode,
    ) -> Vec<Finding> {
        let mut out = FileFindings {
            path: &file.path,
            changed: file.changed.as_deref(),
            ignored: &self.ignored,
            findings: Vec::new(),
        };
        let Some(text) = text::decode(&file.bytes) else {
            out.push(parse_error(
                "the file is not UTF-8, so no rule checked it".into(),
            ));
            return out.findings;
        };
        let Ok(source_type) = SourceType::from_path(&file.path) else {
            out.push(parse_error(
                "no parser for this file extension, so no rule checked it".into(),
            ));
            return out.findings;
        };
        let parsed = Parser::new(allocator, text, source_type)
            .with_options(ParseOptions {
                preserve_parens: false,
                ..ParseOptions::default()
            })
            .parse();
        let lines = LineIndex::new(text);
        // oxc puts at least one diagnostic with a fatal error.
        if let Some(diagnostic) = parsed.diagnostics.first() {
            let span = diagnostic
                .labels
                .first()
                .map_or(Span::empty(0), |label| label.span());
            out.push(Draft {
                start: lines.position(span.start),
                end: lines.position(span.end),
                place: span.source_text(text),
                ..parse_error(diagnostic.to_string())
            });
            return out.findings;
        }
        let role = self.set.roles.role_of(&file.path);
        let profile = self.dispatch.profile(mode, role, facts.package(&file.path));
        // Every rule of a profile is in `on_file`, so an empty one runs no rule.
        if profile.on_file.is_empty() {
            return out.findings;
        }

        let semantic = SemanticBuilder::new()
            .with_build_nodes(true)
            .build(&parsed.program)
            .semantic;
        let source = File {
            path: &file.path,
            text,
            lines: &lines,
            semantic: &semantic,
            role,
        };
        let mut reports: Vec<Vec<Report>> = self.rules.iter().map(|_| Vec::new()).collect();
        let mut functions = Vec::new();
        for node in semantic.nodes().iter() {
            for &r in &profile.by_type[node.kind().ty() as usize] {
                self.rules[r]
                    .1
                    .on_node(node, &mut Cx::new(&source, facts, &mut reports[r]));
            }
            if let Some(name) = finding::function_name(node.kind()) {
                functions.push((node.span(), name));
            }
        }
        for &r in &profile.on_file {
            self.rules[r]
                .1
                .on_file(&mut Cx::new(&source, facts, &mut reports[r]));
        }

        for ((at, _), reports) in self.rules.iter().zip(reports) {
            let meta = &self.set.rules[*at].meta;
            let kind = match meta.kind {
                RuleKind::Error => FindingKind::Error,
                RuleKind::Candidate => FindingKind::Candidate,
                RuleKind::Agent => unreachable!("agent rules are never built"),
            };
            for report in reports {
                out.push(Draft {
                    rule: meta.id,
                    kind,
                    message: finding::fill(meta.message, &report.args),
                    start: lines.position(report.span.start),
                    end: lines.position(report.span.end),
                    fixable: meta.fix && !report.edits.is_empty(),
                    place: report.span.source_text(text),
                    function: finding::function_around(&functions, report.span.start),
                });
            }
        }
        out.findings
    }
}

/// At 1:1, with no text to fingerprint.
fn parse_error(message: String) -> Draft<'static> {
    let start = || Position { line: 1, column: 1 };
    Draft {
        rule: PARSE_ERROR,
        kind: FindingKind::ParseError,
        message,
        start: start(),
        end: start(),
        fixable: false,
        place: "",
        function: "",
    }
}
