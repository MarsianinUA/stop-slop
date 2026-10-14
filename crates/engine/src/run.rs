use camino::Utf8Path;
use kit::oxc::allocator::Allocator;
use kit::oxc::ast::ast::Program;
use kit::oxc::parser::{ParseOptions, Parser};
use kit::oxc::semantic::SemanticBuilder;
use kit::oxc::span::{GetSpan, SourceType, Span};
use kit::{Cx, Facts, File, FindingKind, LineIndex, Position, Report, Role, RuleKind, RuleMeta};
use rayon::prelude::*;

use crate::dispatch::Profile;
use crate::finding::{self, Draft, FileFindings, PARSE_ERROR};
use crate::{Engine, Finding, Mode, RunReport, SourceFile, text};

impl Engine {
    /// The findings are sorted, so the order does not depend on the number of threads.
    pub fn run(&self, files: Vec<SourceFile>, facts: &dyn Facts, mode: Mode) -> RunReport {
        if mode == Mode::Fix {
            let mut fixes: Vec<_> = files
                .into_par_iter()
                .map_init(Allocator::default, |allocator, file| {
                    self.fix_file(allocator, &file, facts)
                })
                .flatten_iter()
                .collect();
            // The counts of a file are sorted by rule, so this sorts `fixed` by file, rule.
            fixes.sort_by(|(a, _), (b, _)| a.path.cmp(&b.path));
            let (rewritten, fixed): (Vec<_>, Vec<_>) = fixes.into_iter().unzip();
            return RunReport {
                findings: Vec::new(),
                fixed: fixed.into_iter().flatten().collect(),
                rewritten,
            };
        }
        let mut findings: Vec<Finding> = files
            .into_par_iter()
            .map_init(Allocator::default, |allocator, file| {
                allocator.reset();
                self.check_file(allocator, &file, facts, mode)
            })
            .flatten_iter()
            .collect();
        finding::sort(&mut findings);
        RunReport {
            findings,
            fixed: Vec::new(),
            rewritten: Vec::new(),
        }
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
        let Some((text, _)) = text::decode(&file.bytes) else {
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
        let program = match parse(allocator, text, source_type) {
            Ok(program) => program,
            Err((span, message)) => {
                let lines = LineIndex::new(text);
                out.push(Draft {
                    start: lines.position(span.start),
                    end: lines.position(span.end),
                    place: span.source_text(text),
                    ..parse_error(message)
                });
                return out.findings;
            }
        };
        let role = self.set.roles.role_of(&file.path);
        let profile = self.dispatch.profile(mode, role, facts.package(&file.path));
        // Every rule of a profile is in `on_file`, so an empty one runs no rule.
        if profile.on_file.is_empty() {
            return out.findings;
        }
        self.run_rules(
            &program,
            &file.path,
            role,
            profile,
            facts,
            |source, meta, report, function| {
                let kind = match meta.kind {
                    RuleKind::Error => FindingKind::Error,
                    RuleKind::Candidate => FindingKind::Candidate,
                    RuleKind::Agent => unreachable!("agent rules are never built"),
                };
                out.push(Draft {
                    rule: meta.id,
                    kind,
                    message: finding::fill(meta.message, &report.args),
                    start: source.lines.position(report.span.start),
                    end: source.lines.position(report.span.end),
                    fixable: fixable(meta, &report),
                    place: report.span.source_text(source.text),
                    function,
                });
            },
        );
        out.findings
    }

    /// Runs the rules of `profile` on `program` and hands `each` every report, rule by rule in
    /// the order of `self.rules`, with the name of the function around the report.
    pub(crate) fn run_rules(
        &self,
        program: &Program<'_>,
        path: &Utf8Path,
        role: Option<Role>,
        profile: &Profile,
        facts: &dyn Facts,
        mut each: impl FnMut(&File, &'static RuleMeta, Report, &str),
    ) {
        let text = program.source_text;
        let lines = LineIndex::new(text);
        let semantic = SemanticBuilder::new()
            .with_build_nodes(true)
            .build(program)
            .semantic;
        let source = File {
            path,
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
            for report in reports {
                let function = finding::function_around(&functions, report.span.start);
                each(&source, meta, report, function);
            }
        }
    }
}

/// `Check` marks and `Fix` applies the same reports.
pub(crate) fn fixable(meta: &RuleMeta, report: &Report) -> bool {
    meta.fix && !report.edits.is_empty()
}

/// The span and message of the first diagnostic when `text` does not parse.
pub(crate) fn parse<'a>(
    allocator: &'a Allocator,
    text: &'a str,
    source_type: SourceType,
) -> Result<Program<'a>, (Span, String)> {
    let parsed = Parser::new(allocator, text, source_type)
        .with_options(ParseOptions {
            preserve_parens: false,
            ..ParseOptions::default()
        })
        .parse();
    // oxc puts at least one diagnostic with a fatal error.
    match parsed.diagnostics.first() {
        None => Ok(parsed.program),
        Some(diagnostic) => {
            let span = diagnostic
                .labels
                .first()
                .map_or(Span::empty(0), |label| label.span());
            Err((span, diagnostic.to_string()))
        }
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
