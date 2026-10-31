use camino::Utf8Path;
use kit::oxc::allocator::Allocator;
use kit::oxc::ast::ast::Program;
use kit::oxc::parser::{ParseOptions, Parser};
use kit::oxc::semantic::SemanticBuilder;
use kit::oxc::span::{GetSpan, SourceType, Span};
use kit::{Cx, Facts, File, FindingKind, LineIndex, Position, Report, Role, RuleKind, RuleMeta};
use rayon::prelude::*;

use crate::dispatch::Profile;
use crate::finding::{self, Draft, FileFindings, Owner, PARSE_ERROR};
use crate::place::Places;
use crate::{Engine, Finding, Mode, RunReport, SourceFile, fix, text};

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
        let changed = file.changed.as_ref();
        let mut out = FileFindings {
            path: &file.path,
            changed,
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
        if profile.rules.is_empty() {
            return out.findings;
        }
        // Built at the first report: most files have none.
        let mut places: Option<Places> = None;
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
                let owner = if report.on_path {
                    Owner::Path
                } else {
                    let places = places.get_or_insert_with(|| {
                        Places::new(source.semantic, text, changed.is_some())
                    });
                    let (name, touched) = places.locate(report.span, source.lines, changed);
                    Owner::Code { name, touched }
                };
                out.push(Draft {
                    rule: meta.id,
                    kind,
                    message: finding::fill(meta.message, &report.args),
                    args: finding::named(meta.message, report.args),
                    owner,
                    start: source.lines.position(report.span.start),
                    end: source.lines.position(report.span.end),
                    fixable: fix::sound_edits(
                        meta,
                        report.edits,
                        source.text,
                        source.lines.line_ending(),
                    )
                    .is_some(),
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
        for &r in &profile.rules {
            self.rules[r]
                .1
                .on_file(&mut Cx::new(&source, facts, &mut reports[r]));
        }

        for ((def, _), reports) in self.rules.iter().zip(reports) {
            for mut report in reports {
                report.span = within(report.span, text);
                let function = finding::function_around(&functions, report.span.start);
                each(&source, &def.meta, report, function);
            }
        }
    }
}

/// `span` cut to fit `text` on char boundaries. A span past the end, inside a char or
/// backwards would panic every slice of the text and stop the whole run; cut, the finding
/// still shows near the place the rule meant.
fn within(span: Span, text: &str) -> Span {
    let floor = |offset: u32| text.floor_char_boundary(offset as usize) as u32;
    let end = floor(span.end);
    Span::new(floor(span.start).min(end), end)
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
        args: vec![("error", message.clone())],
        message,
        owner: Owner::Text,
        start: start(),
        end: start(),
        fixable: false,
        place: "",
        function: "",
    }
}
