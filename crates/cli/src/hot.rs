//! `stop-slop hot`: the hot functions of the branch in the `report.json` of `stop-slop-stand run`,
//! with the findings of `check` on their lines, and the changed functions the coverage repeat never
//! entered. The report is read as JSON: the linter takes no crate of the stand.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::iter;
use std::path::Path;

use camino::{Utf8Path, Utf8PathBuf};
use engine::{Engine, Finding, FunctionSpan, LineRange, SourceFile};
use kit::serde::Deserialize;
use kit::{Position, Role, Roles};
use serde_json::Value;
use state::State;

use crate::args::{CheckArgs, HotArgs};
use crate::check;
use crate::error::{CliError, Exit};
use crate::output::{self, JsonLayout};

const SCHEMA_VERSION: u64 = 1;

/// The fields of `report.json` that `hot` reads; the others are skipped.
#[derive(Deserialize)]
#[serde(crate = "kit::serde")]
struct Report {
    head: Head,
    hot: Vec<Sampled>,
    coverage: Option<CoverageJson>,
    flags: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(crate = "kit::serde")]
struct Head {
    tree: String,
}

/// A hot function of the report. `line` and `column`, from 1, are where V8 starts it; the column
/// counts UTF-16 code units, as V8 and source maps do.
#[derive(Deserialize)]
#[serde(crate = "kit::serde", rename_all = "camelCase")]
struct Sampled {
    service: String,
    file: String,
    /// Empty for an anonymous function.
    name: String,
    line: u32,
    column: u32,
    self_ms: f64,
    total_ms: f64,
}

#[derive(Deserialize)]
#[serde(crate = "kit::serde")]
struct CoverageJson {
    called: Vec<Called>,
}

/// A function the coverage repeat entered, its start counted as in `Sampled`; V8 starts a
/// function there at another place than in the profile.
#[derive(Deserialize)]
#[serde(crate = "kit::serde")]
struct Called {
    file: String,
    line: u32,
    column: u32,
    /// Entries into the body, each resumption after an `await` too.
    calls: u64,
}

pub(crate) struct Found<'c> {
    /// The hottest first.
    pub(crate) hot: Vec<HotRow<'c>>,
    pub(crate) coverage: Coverage<'c>,
    /// The branch has files outside tests, which the stand may run.
    pub(crate) app_files: bool,
}

pub(crate) struct HotRow<'c> {
    pub(crate) service: String,
    pub(crate) file: &'c Utf8Path,
    pub(crate) function: Option<String>,
    /// The lines of the function on disk now; V8's line of the report when no function is
    /// around its start any more.
    pub(crate) line: u32,
    pub(crate) end_line: u32,
    pub(crate) self_ms: f64,
    pub(crate) total_ms: f64,
    /// `None` without coverage, or when no function of the file is around the start any more.
    pub(crate) entries: Option<u64>,
    /// The file differs from the head tree of the report, so its lines may have moved since.
    pub(crate) stale: bool,
    /// Those on the lines of the function.
    pub(crate) findings: Vec<&'c Finding>,
}

pub(crate) struct Unexercised<'c> {
    pub(crate) file: &'c Utf8Path,
    pub(crate) function: Option<String>,
    pub(crate) line: u32,
    pub(crate) end_line: u32,
    pub(crate) stale: bool,
}

pub(crate) enum Coverage<'c> {
    /// With the changed functions the coverage repeat never entered.
    Collected(Vec<Unexercised<'c>>),
    /// No service had a profile, or the report does not say why there is none.
    NotCollected,
    /// The reasons of its `coverage-unknown` flags.
    Unknown(Vec<String>),
}

impl Found<'_> {
    fn exit(&self) -> Exit {
        let listed = match &self.coverage {
            Coverage::Collected(unexercised) => !unexercised.is_empty(),
            // Without coverage, code of the branch may never have run, and nothing can tell which.
            Coverage::NotCollected | Coverage::Unknown(_) => self.app_files,
        };
        if !self.hot.is_empty() || listed {
            Exit::Found
        } else {
            Exit::Clean
        }
    }
}

pub(crate) fn run(args: HotArgs) -> Result<Exit, CliError> {
    let report = load(&args.report)?;
    let repo = crate::cwd_repo()?;
    if !repo.has_tree(&report.head.tree)? {
        let message = format!(
            "head.tree {} is not a tree of this repository",
            report.head.tree
        );
        return Err(CliError::Report {
            path: args.report,
            message,
        });
    }
    let state = State::open(repo.git_common_dir());
    let engine = crate::engine_for(&rules::RULESET, &state)?;
    let scan = CheckArgs {
        base: args.base,
        ..CheckArgs::default()
    };
    let checked = check::scan(&repo, &engine, scan)?;
    let stale = repo.changed_since(&report.head.tree, &checked.files)?;
    let bytes = repo.worktree().read(&checked.files)?;
    let called = report
        .coverage
        .as_ref()
        .map(|coverage| &coverage.called[..]);
    let sources: Vec<Source<'_>> = checked
        .files
        .iter()
        .zip(bytes)
        .map(|(path, bytes)| Source::new(path, bytes, called))
        .collect();
    let roles = &rules::RULESET.roles;
    let coverage = match called {
        Some(_) => Coverage::Collected(unexercised(&sources, &checked.changed, &stale, roles)),
        None => no_coverage(&report.flags),
    };
    let found = Found {
        hot: rows(&sources, &report.hot, &checked.findings, &stale),
        coverage,
        app_files: checked.files.iter().any(|path| !is_test(roles, path)),
    };
    let base = checked.base.as_ref();
    let text = if args.json {
        output::hot_json(base, &found, &checked.rules, JsonLayout::new(args.pretty))
    } else {
        output::hot_text(base, &found)
    };
    output::print(&text)?;
    Ok(found.exit())
}

fn load(path: &Path) -> Result<Report, CliError> {
    let refuse = |message: String| CliError::Report {
        path: path.to_owned(),
        message,
    };
    let text = fs::read_to_string(path).map_err(|error| refuse(error.to_string()))?;
    parse(&text).map_err(refuse)
}

/// `schemaVersion` first: a report of another version is named so, not by a field it lacks.
fn parse(text: &str) -> Result<Report, String> {
    #[derive(Deserialize)]
    #[serde(crate = "kit::serde", rename_all = "camelCase")]
    struct Version {
        schema_version: u64,
    }
    let version: Version = serde_json::from_str(text).map_err(|error| error.to_string())?;
    if version.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "schemaVersion {} is not {SCHEMA_VERSION}, the one this stop-slop reads",
            version.schema_version
        ));
    }
    serde_json::from_str(text).map_err(|error| error.to_string())
}

fn no_coverage(flags: &[Value]) -> Coverage<'static> {
    let reasons: Vec<String> = flags
        .iter()
        .filter(|flag| flag["kind"] == "coverage-unknown")
        .map(unknown_reason)
        .collect();
    if reasons.is_empty() {
        Coverage::NotCollected
    } else {
        Coverage::Unknown(reasons)
    }
}

/// A `coverage-unknown` flag in words, as `guide line` tells it.
fn unknown_reason(flag: &Value) -> String {
    match (flag["stage"].as_str(), flag["service"].as_str()) {
        (Some(stage), _) => format!("the coverage repeat failed at {stage}"),
        (None, Some(service)) => format!("{service} left no coverage"),
        (None, None) => "the coverage repeat covered no service".to_owned(),
    }
}

/// A file of the branch as it is on disk now.
struct Source<'c> {
    path: &'c Utf8Path,
    /// Without a BOM: `tsc` and Node drop it before V8 counts.
    text: String,
    /// The byte offset where each line starts.
    lines: Vec<usize>,
    spans: Vec<FunctionSpan>,
    /// Entries into each of `spans`; `None` without coverage.
    entries: Option<Vec<u64>>,
}

impl<'c> Source<'c> {
    /// `bytes` is `None` for a file gone from the disk since `check` read it.
    fn new(path: &'c Utf8Path, bytes: Option<Vec<u8>>, called: Option<&[Called]>) -> Self {
        let file = SourceFile {
            path: path.to_owned(),
            bytes: bytes.unwrap_or_default(),
            changed: None,
        };
        let spans = Engine::functions(&file);
        let text = String::from_utf8(file.bytes).unwrap_or_default();
        let text = match text.strip_prefix('\u{feff}') {
            Some(rest) => rest.to_owned(),
            None => text,
        };
        let lines = iter::once(0)
            .chain(text.match_indices('\n').map(|(at, _)| at + 1))
            .collect();
        let mut source = Source {
            path,
            text,
            lines,
            spans,
            entries: None,
        };
        let entries = called.map(|starts| {
            let mut entries = vec![0_u64; source.spans.len()];
            let here = starts.iter().filter(|start| start.file == path.as_str());
            for start in here {
                if let Some(index) = source.innermost(start.line, start.column) {
                    entries[index] = entries[index].saturating_add(start.calls);
                }
            }
            entries
        });
        source.entries = entries;
        source
    }

    /// The index of the innermost span around a start of V8.
    fn innermost(&self, line: u32, utf16_column: u32) -> Option<usize> {
        let at = Position {
            line,
            column: self.char_column(line, utf16_column),
        };
        // Spans come outermost first, so the last one around `at` is the innermost.
        self.spans
            .iter()
            .rposition(|span| span.start <= at && at < span.end)
    }

    /// The column in characters, as `Position` counts, of a column in UTF-16 code units: they
    /// differ after a character outside the BMP, such as an emoji.
    fn char_column(&self, line: u32, utf16_column: u32) -> u32 {
        let start = line
            .checked_sub(1)
            .and_then(|index| self.lines.get(index as usize));
        let Some(&start) = start else {
            return utf16_column;
        };
        let units = utf16_column.saturating_sub(1) as usize;
        let (mut seen, mut chars) = (0, 0);
        for ch in self.text[start..].chars().take_while(|&ch| ch != '\n') {
            if seen >= units {
                break;
            }
            seen += ch.len_utf16();
            chars += 1;
        }
        // Past the end of the line, a unit per character.
        (chars + units.saturating_sub(seen) + 1) as u32
    }
}

fn rows<'c>(
    sources: &[Source<'c>],
    hot: &[Sampled],
    findings: &'c [Finding],
    stale: &BTreeSet<Utf8PathBuf>,
) -> Vec<HotRow<'c>> {
    let mut rows: Vec<HotRow<'c>> = hot
        .iter()
        .filter_map(|sampled| {
            let source = sources
                .iter()
                .find(|source| source.path.as_str() == sampled.file)?;
            let (function, line, end_line, entries) =
                match source.innermost(sampled.line, sampled.column) {
                    Some(index) => {
                        let span = &source.spans[index];
                        let entries = source.entries.as_ref().map(|entries| entries[index]);
                        (span.name.clone(), span.start.line, span.end.line, entries)
                    }
                    // A file edited since the run may have no function there any more.
                    None => {
                        let name = (!sampled.name.is_empty()).then(|| sampled.name.clone());
                        (name, sampled.line, sampled.line, None)
                    }
                };
            let inside = |finding: &&Finding| {
                finding.file.as_path() == source.path
                    && (line..=end_line).contains(&finding.start.line)
            };
            Some(HotRow {
                service: sampled.service.clone(),
                file: source.path,
                function,
                line,
                end_line,
                self_ms: sampled.self_ms,
                total_ms: sampled.total_ms,
                entries,
                stale: stale.contains(source.path),
                findings: findings.iter().filter(inside).collect(),
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        let hotter = b.self_ms.total_cmp(&a.self_ms);
        hotter
            .then_with(|| a.file.cmp(b.file))
            .then(a.line.cmp(&b.line))
    });
    rows
}

/// The functions on changed lines that the coverage repeat never entered, outside test files:
/// the stand runs the app, not its tests. A function inside a listed one never ran either, so
/// it is not listed again.
fn unexercised<'c>(
    sources: &[Source<'c>],
    changed: &BTreeMap<Utf8PathBuf, Vec<LineRange>>,
    stale: &BTreeSet<Utf8PathBuf>,
    roles: &Roles,
) -> Vec<Unexercised<'c>> {
    let mut functions = Vec::new();
    for source in sources {
        let (Some(entries), Some(ranges)) = (&source.entries, changed.get(source.path)) else {
            continue;
        };
        if is_test(roles, source.path) {
            continue;
        }
        let mut listed_end: Option<&Position> = None;
        for (span, &entries) in source.spans.iter().zip(entries) {
            // Spans come outermost first, so one inside a listed span follows it.
            if listed_end.is_some_and(|end| span.start < *end) {
                continue;
            }
            let touched = ranges
                .iter()
                .any(|range| range.start <= span.end.line && span.start.line <= range.end);
            if touched && entries == 0 {
                listed_end = Some(&span.end);
                functions.push(Unexercised {
                    file: source.path,
                    function: span.name.clone(),
                    line: span.start.line,
                    end_line: span.end.line,
                    stale: stale.contains(source.path),
                });
            }
        }
    }
    functions
}

fn is_test(roles: &Roles, path: &Utf8Path) -> bool {
    matches!(roles.role_of(path), Some(Role::Tests))
}

#[cfg(test)]
mod tests {
    use engine::{Mode, RunConfig};
    use kit::{EnumIndex, StaticFacts};

    use super::*;

    fn source<'p>(path: &'p str, text: &str, called: Option<&[Called]>) -> Source<'p> {
        Source::new(Utf8Path::new(path), Some(text.as_bytes().to_vec()), called)
    }

    /// Anonymous to V8, so a row gets its name only from the span it matched.
    fn sampled(service: &str, file: &str, (line, column): (u32, u32), self_ms: f64) -> Sampled {
        Sampled {
            service: service.to_owned(),
            file: file.to_owned(),
            name: String::new(),
            line,
            column,
            self_ms,
            total_ms: self_ms,
        }
    }

    fn called(file: &str, (line, column): (u32, u32), calls: u64) -> Called {
        Called {
            file: file.to_owned(),
            line,
            column,
            calls,
        }
    }

    fn places<'r>(rows: &'r [HotRow<'_>]) -> Vec<(Option<&'r str>, u32, u32)> {
        rows.iter()
            .map(|row| (row.function.as_deref(), row.line, row.end_line))
            .collect()
    }

    /// The findings of every rule that is on by default.
    fn findings(path: &str, text: &str) -> Vec<Finding> {
        let engine = Engine::new(&rules::RULESET, RunConfig::default()).unwrap();
        let facts = StaticFacts {
            package: None,
            aliases: Vec::new(),
            enums: EnumIndex::new(Vec::new()),
        };
        let file = SourceFile {
            path: path.into(),
            bytes: text.as_bytes().to_vec(),
            changed: None,
        };
        engine.run(vec![file], &facts, Mode::Check).findings
    }

    const ORDERS: &str = "class Orders {\n  list(rows: number[]) {\n    return rows.map((row) => row + 1);\n  }\n}\n";

    #[test]
    fn a_start_in_a_method_goes_to_the_method_and_in_a_nested_function_to_the_innermost() {
        let file = source("src/orders.ts", ORDERS, None);
        let hot = [
            // The `(` of `list`, as the profile has it.
            sampled("api", "src/orders.ts", (2, 7), 4.0),
            // The `(` of the arrow: its first character.
            sampled("api", "src/orders.ts", (3, 21), 3.0),
            // The `)` of `map`, just past the arrow.
            sampled("api", "src/orders.ts", (3, 37), 2.0),
        ];

        let rows = rows(&[file], &hot, &[], &BTreeSet::new());
        assert_eq!(
            places(&rows),
            [
                (Some("list"), 2, 4),
                (Some("list"), 3, 3),
                (Some("list"), 2, 4)
            ]
        );
    }

    #[test]
    fn one_function_of_two_services_gives_two_rows_marked_stale_with_their_file() {
        let files = [
            source("src/orders.ts", ORDERS, None),
            source("src/other.ts", ORDERS, None),
        ];
        let hot = [
            sampled("api", "src/orders.ts", (2, 7), 4.0),
            sampled("worker", "src/orders.ts", (2, 7), 3.0),
            sampled("api", "src/other.ts", (2, 7), 2.0),
            sampled("api", "src/outside.ts", (2, 7), 1.0),
        ];
        let stale = BTreeSet::from([Utf8PathBuf::from("src/orders.ts")]);

        let rows = rows(&files, &hot, &[], &stale);
        let seen: Vec<(&str, &str, bool)> = rows
            .iter()
            .map(|row| (row.service.as_str(), row.file.as_str(), row.stale))
            .collect();
        assert_eq!(
            seen,
            [
                ("api", "src/orders.ts", true),
                ("worker", "src/orders.ts", true),
                ("api", "src/other.ts", false)
            ]
        );
    }

    #[test]
    fn a_row_holds_only_the_findings_on_the_lines_of_its_function() {
        let text = "export const loose: any = 1;\nexport function sum(values: number[]): number {\n  const first: any = values[0];\n  return first;\n}\nexport const late: any = 2;\n";
        let all = findings("src/sum.ts", text);
        let lines_of = |rule: &str, findings: &[&Finding]| -> Vec<u32> {
            let of_rule = findings.iter().filter(|finding| finding.rule == rule);
            of_rule.map(|finding| finding.start.line).collect()
        };
        assert_eq!(
            lines_of("no-any", &all.iter().collect::<Vec<_>>()),
            [1, 3, 6]
        );

        let file = source("src/sum.ts", text, None);
        let hot = [sampled("api", "src/sum.ts", (2, 20), 1.0)];
        let rows = rows(&[file], &hot, &all, &BTreeSet::new());
        let [row] = rows.as_slice() else {
            panic!("one row");
        };
        assert_eq!((row.line, row.end_line), (2, 5));
        assert_eq!(lines_of("no-any", &row.findings), [3]);
        assert!(
            row.findings
                .iter()
                .all(|finding| (2..=5).contains(&finding.start.line))
        );
    }

    #[test]
    fn rows_go_by_self_time_the_hottest_first() {
        let text = "export function a() {}\nexport function b() {}\nexport function c() {}\n";
        let file = source("src/abc.ts", text, None);
        let hot = [
            sampled("api", "src/abc.ts", (1, 18), 5.0),
            sampled("api", "src/abc.ts", (2, 18), 50.0),
            sampled("api", "src/abc.ts", (3, 18), 20.0),
        ];

        let rows = rows(&[file], &hot, &[], &BTreeSet::new());
        let names: Vec<Option<&str>> = rows.iter().map(|row| row.function.as_deref()).collect();
        assert_eq!(names, [Some("b"), Some("c"), Some("a")]);
    }

    #[test]
    fn a_utf16_column_after_wide_characters_finds_the_function_there() {
        // `é` is 2 bytes and 1 UTF-16 unit, each emoji 4 bytes and 2 units, all 1 character:
        // V8 puts `a` at unit 32 and `b` at 43, which are characters 26 and 37.
        let text = "const s = \"é😀😀😀😀😀😀\", a = ()=>0, b = ()=>1;\n";
        let file = source("src/wide.ts", text, None);
        let columns: Vec<(u32, u32)> = file
            .spans
            .iter()
            .map(|span| (span.start.column, span.end.column))
            .collect();
        assert_eq!(columns, [(26, 31), (37, 42)]);
        let hot = [
            sampled("api", "src/wide.ts", (1, 32), 2.0),
            sampled("api", "src/wide.ts", (1, 43), 1.0),
        ];

        let rows = rows(&[file], &hot, &[], &BTreeSet::new());
        assert_eq!(places(&rows), [(Some("a"), 1, 1), (Some("b"), 1, 1)]);
    }

    #[test]
    fn unexercised_lists_changed_functions_without_entries_outside_tests() {
        let text = "export function early() {\n  return 1;\n}\nexport function used() {\n  return 2;\n}\nexport function late() {\n  return 3;\n}\nexport function old() {\n  return 4;\n}\n";
        // Coverage starts `used` at its `export`, as the source map of `tsc` puts it.
        let entered = [
            called("src/a.ts", (4, 1), 3),
            called("src/a.spec.ts", (4, 1), 3),
        ];
        let files = [
            source("src/a.ts", text, Some(&entered)),
            source("src/a.spec.ts", text, Some(&entered)),
            source("src/b.ts", text, None),
        ];
        assert_eq!(files[0].entries, Some(vec![0, 3, 0, 0]));
        // The last line of `early`, the first of `used` and of `late`.
        let lines = vec![
            LineRange { start: 3, end: 4 },
            LineRange { start: 7, end: 7 },
        ];
        let changed = BTreeMap::from([
            (Utf8PathBuf::from("src/a.ts"), lines.clone()),
            (Utf8PathBuf::from("src/a.spec.ts"), lines.clone()),
            (Utf8PathBuf::from("src/b.ts"), lines),
        ]);

        let functions = unexercised(&files, &changed, &BTreeSet::new(), &rules::RULESET.roles);
        let seen: Vec<(&str, Option<&str>, u32, u32)> = functions
            .iter()
            .map(|f| (f.file.as_str(), f.function.as_deref(), f.line, f.end_line))
            .collect();
        assert_eq!(
            seen,
            [
                ("src/a.ts", Some("early"), 1, 3),
                ("src/a.ts", Some("late"), 7, 9)
            ]
        );
    }

    #[test]
    fn unexercised_lists_a_function_once_without_its_callbacks_but_a_callback_of_one_that_ran() {
        let text = "export function outer(xs: number[]) {\n  return xs.map((x) => x).filter((x) => x > 0);\n}\nexport function inner(xs: number[]) {\n  return xs.map((x) => x + 1);\n}\n";
        let entered = [called("src/a.ts", (4, 1), 1)];
        let files = [source("src/a.ts", text, Some(&entered))];
        assert_eq!(files[0].entries, Some(vec![0, 0, 0, 1, 0]));
        let lines = vec![LineRange { start: 1, end: 6 }];
        let changed = BTreeMap::from([(Utf8PathBuf::from("src/a.ts"), lines)]);

        let functions = unexercised(&files, &changed, &BTreeSet::new(), &rules::RULESET.roles);
        let seen: Vec<(Option<&str>, u32, u32)> = functions
            .iter()
            .map(|f| (f.function.as_deref(), f.line, f.end_line))
            .collect();
        assert_eq!(seen, [(Some("outer"), 1, 3), (Some("inner"), 5, 5)]);
    }

    #[test]
    fn a_hot_start_outside_every_function_keeps_the_name_and_line_of_the_report() {
        // Line 2 was a function at the run and is blank now.
        let text = "export function a() {}\n\nexport function b() {}\n";
        let entered = [called("src/ab.ts", (2, 1), 5)];
        let file = source("src/ab.ts", text, Some(&entered));
        let gone = Sampled {
            name: "gone".to_owned(),
            ..sampled("api", "src/ab.ts", (2, 1), 1.0)
        };

        let rows = rows(&[file], &[gone], &[], &BTreeSet::new());
        assert_eq!(places(&rows), [(Some("gone"), 2, 2)]);
        assert_eq!(rows[0].entries, None);
    }

    #[test]
    fn coverage_tells_not_collected_from_unknown_with_the_reasons() {
        let report = |flags: &str| {
            let text = format!(
                r#"{{"schemaVersion": 1, "head": {{"tree": "t"}}, "hot": [], "coverage": null, "flags": {flags}}}"#
            );
            no_coverage(&parse(&text).unwrap().flags)
        };
        assert!(matches!(
            report(r#"[{"kind": "unprofiled"}]"#),
            Coverage::NotCollected
        ));
        let flags = r#"[
            {"kind": "coverage-unknown", "stage": "setup", "service": null},
            {"kind": "no-profile", "service": "db"},
            {"kind": "coverage-unknown", "stage": null, "service": "api"},
            {"kind": "coverage-unknown", "stage": null, "service": null}
        ]"#;
        let Coverage::Unknown(reasons) = report(flags) else {
            panic!("unknown");
        };
        assert_eq!(
            reasons,
            [
                "the coverage repeat failed at setup",
                "api left no coverage",
                "the coverage repeat covered no service"
            ]
        );
    }

    #[test]
    fn parse_names_another_schema_version_before_a_missing_field() {
        let error = parse(r#"{"schemaVersion": 2}"#).err().unwrap();
        assert_eq!(
            error,
            "schemaVersion 2 is not 1, the one this stop-slop reads"
        );
        let error = parse(r#"{"schemaVersion": 1}"#).err().unwrap();
        assert!(error.contains("missing field `head`"), "{error}");
    }
}
