use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};
use engine::{
    Changed, ConfigError, Cut, Engine, Finding, FixCount, LineRange, Mode, RunConfig, RunReport,
    SourceFile,
};
use kit::prelude::*;
use kit::{
    EnumIndex, Facts, Files, FindingKind, GroupDef, Impl, Package, RoleDef, Roles, RuleDef,
    RuleKind, RuleMeta, Ruleset, Source, StaticFacts, build,
};

/// Names each node it is given, and reports once per file.
#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
struct Seen {}

impl Rule for Seen {
    const NODES: &'static [AstType] = &[AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        cx.report(node.span())
            .arg("what", format!("{:?}", node.kind().ty()));
    }

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        cx.report(Span::empty(0)).arg("what", "file");
    }
}

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
struct PerFile {}

impl Rule for PerFile {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        cx.report(Span::empty(0));
    }
}

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
struct Comments {}

impl Rule for Comments {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        for comment in cx.file().semantic.comments() {
            cx.report(comment.span);
        }
    }
}

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
struct OnPath {}

impl Rule for OnPath {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        cx.report_path();
    }
}

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
struct Depth {
    depth: u32,
}

impl Rule for Depth {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        cx.report(Span::empty(0))
            .arg("name", "{depth}")
            .arg("depth", self.depth);
    }
}

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
struct UsesAliases {}

impl Rule for UsesAliases {
    const USES: &'static [Fact] = &[Fact::Aliases];

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        cx.report(Span::empty(0));
    }
}

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
struct UsesEnums {}

impl Rule for UsesEnums {
    const USES: &'static [Fact] = &[Fact::Enums];

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        cx.report(Span::empty(0));
    }
}

/// Reports every call; with an edit when the callee is `edit`.
#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
struct Fixer {}

impl Rule for Fixer {
    const NODES: &'static [AstType] = &[AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::CallExpression(call) = node.kind() else {
            return;
        };
        let reported = cx.report(call.span);
        if call.callee_name() == Some("edit") {
            reported.fix([Edit {
                span: call.span,
                text: String::new(),
            }]);
        }
    }
}

/// `wrap(x)` -> `x`.
#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
struct Unwrap {}

impl Rule for Unwrap {
    const NODES: &'static [AstType] = &[AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::CallExpression(call) = node.kind() else {
            return;
        };
        if call.callee_name() == Some("wrap")
            && let [argument] = call.arguments.as_slice()
        {
            let text = argument.span().source_text(cx.file().text).to_string();
            cx.report(call.span).fix([Edit {
                span: call.span,
                text,
            }]);
        }
    }
}

/// `pair()` -> `a();`, then `b()` on the next line, the two joined by `ending`.
#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
struct Split {
    ending: String,
}

impl Rule for Split {
    const NODES: &'static [AstType] = &[AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::CallExpression(call) = node.kind() else {
            return;
        };
        if call.callee_name() == Some("pair") {
            cx.report(call.span).fix([Edit {
                span: call.span,
                text: format!("a();{}b()", self.ending),
            }]);
        }
    }
}

/// Each `breaks()` adds `(` at the end of the file, which then does not parse.
#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
struct Break {}

impl Rule for Break {
    const NODES: &'static [AstType] = &[AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::CallExpression(call) = node.kind() else {
            return;
        };
        if call.callee_name() == Some("breaks") {
            let end = cx.file().text.len() as u32;
            cx.report(call.span).fix([Edit {
                span: Span::empty(end),
                text: "(".into(),
            }]);
        }
    }
}

/// Sound findings with broken edits.
#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
struct BadEdits {}

impl Rule for BadEdits {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let text = cx.file().text;
        let edit = |start: u32, end: u32| Edit {
            span: Span::new(start, end),
            text: "x".into(),
        };
        let past_end = text.len() as u32 + 1;
        cx.report(Span::empty(0)).fix([edit(past_end, past_end)]);
        cx.report(Span::empty(0)).fix([edit(2, 1)]);
        cx.report(Span::empty(0)).fix([edit(0, 2), edit(1, 3)]);
        if let Some(at) = text.find('é') {
            let inside = at as u32 + 1;
            cx.report(Span::empty(0)).fix([edit(inside, inside)]);
        }
    }
}

/// Spans past the end of the text, inside `é` and backwards. The first one also puts `0;`
/// at the start of a text that does not have it yet.
#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
struct BadSpans {}

impl Rule for BadSpans {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let text = cx.file().text;
        let end = text.len() as u32;
        let past_end = cx.report(Span::new(end, end + 5));
        if !text.starts_with("0;") {
            past_end.fix([Edit {
                span: Span::empty(0),
                text: "0;".into(),
            }]);
        }
        if let Some(at) = text.find('é') {
            cx.report(Span::empty(at as u32 + 1));
        }
        cx.report(Span::new(2, 1));
    }
}

/// `wrap(x)` -> `wrap(x)`: an edit that changes nothing.
#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
struct Same {}

impl Rule for Same {
    const NODES: &'static [AstType] = &[AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::CallExpression(call) = node.kind() else {
            return;
        };
        if call.callee_name() == Some("wrap") {
            let text = call.span.source_text(cx.file().text).to_string();
            cx.report(call.span).fix([Edit {
                span: call.span,
                text,
            }]);
        }
    }
}

const META: RuleMeta = RuleMeta {
    id: "",
    group: "core",
    description: "",
    kind: RuleKind::Error,
    fix: false,
    files: Files::All,
    source: &[Source::Personal],
    message: "found",
    agent: None,
    options: "",
};

const fn check<R: Rule>(meta: RuleMeta) -> RuleDef {
    RuleDef {
        meta,
        imp: Impl::Check {
            nodes: R::NODES,
            build: build::<R>,
        },
    }
}

const NO_ROLE: RoleDef = RoleDef {
    suffixes: &[],
    dirs: &[],
};

static SET: Ruleset = Ruleset {
    rules: &[
        check::<Fixer>(RuleMeta { id: "call", ..META }),
        check::<Seen>(RuleMeta {
            id: "seen",
            message: "{what}",
            ..META
        }),
        check::<PerFile>(RuleMeta {
            id: "per-file",
            ..META
        }),
        check::<OnPath>(RuleMeta {
            id: "on-path",
            ..META
        }),
        check::<Comments>(RuleMeta {
            id: "comment",
            ..META
        }),
        check::<PerFile>(RuleMeta {
            id: "in-tests",
            files: Files::Tests,
            ..META
        }),
        check::<PerFile>(RuleMeta {
            id: "in-code",
            files: Files::Code,
            ..META
        }),
        check::<Depth>(RuleMeta {
            id: "depth",
            message: "{name} deeper than {depth}",
            options: "depth = 3\n",
            ..META
        }),
        check::<UsesEnums>(RuleMeta {
            id: "candidate",
            kind: RuleKind::Candidate,
            ..META
        }),
        check::<UsesAliases>(RuleMeta {
            id: "nest",
            group: "nestjs",
            ..META
        }),
        check::<Fixer>(RuleMeta {
            id: "fix-on",
            fix: true,
            ..META
        }),
        // Before `unwrap`, so its fix of a `wrap(..)` comes first in a pass.
        check::<Same>(RuleMeta {
            id: "same",
            fix: true,
            ..META
        }),
        check::<Unwrap>(RuleMeta {
            id: "unwrap",
            fix: true,
            ..META
        }),
        check::<Split>(RuleMeta {
            id: "split",
            fix: true,
            options: "ending = \"\\n\"\n",
            ..META
        }),
        check::<Break>(RuleMeta {
            id: "break",
            fix: true,
            ..META
        }),
        check::<BadEdits>(RuleMeta {
            id: "bad-edits",
            fix: true,
            ..META
        }),
        check::<BadSpans>(RuleMeta {
            id: "bad-spans",
            fix: true,
            ..META
        }),
        RuleDef {
            meta: RuleMeta {
                id: "agent",
                kind: RuleKind::Agent,
                agent: Some("look"),
                ..META
            },
            imp: Impl::Agent,
        },
        check::<PerFile>(RuleMeta {
            id: "extra-rule",
            group: "extra",
            ..META
        }),
    ],
    groups: &[
        GroupDef {
            name: "core",
            requires_package: None,
            opt_in: false,
        },
        GroupDef {
            name: "nestjs",
            requires_package: Some("@nestjs/core"),
            opt_in: false,
        },
        GroupDef {
            name: "extra",
            requires_package: None,
            opt_in: true,
        },
    ],
    roles: Roles {
        tests: RoleDef {
            suffixes: &["spec"],
            dirs: &[],
        },
        types: NO_ROLE,
        constants: NO_ROLE,
        functions: NO_ROLE,
    },
};

/// Every other rule of `SET` disabled.
fn only(ids: &[&str]) -> RunConfig {
    RunConfig {
        disabled: SET
            .rules
            .iter()
            .map(|def| def.meta.id)
            .filter(|id| !ids.contains(id))
            .map(String::from)
            .collect(),
        ..RunConfig::default()
    }
}

fn engine(ids: &[&str]) -> Engine {
    Engine::new(&SET, only(ids)).unwrap()
}

fn no_facts() -> StaticFacts {
    StaticFacts {
        package: None,
        aliases: Vec::new(),
        enums: EnumIndex::new(Vec::new()),
    }
}

fn package(dependency: &str) -> Package {
    Package {
        dependencies: BTreeSet::from([dependency.to_string()]),
        workspace: BTreeSet::new(),
    }
}

fn source(path: &str, text: &str) -> SourceFile {
    SourceFile {
        path: path.into(),
        bytes: text.as_bytes().to_vec(),
        changed: None,
    }
}

fn check_files(engine: &Engine, files: Vec<SourceFile>) -> Vec<Finding> {
    engine.run(files, &no_facts(), Mode::Check).findings
}

/// `file:line:column rule` of each finding.
fn brief(findings: &[Finding]) -> Vec<String> {
    findings
        .iter()
        .map(|f| format!("{}:{}:{} {}", f.file, f.start.line, f.start.column, f.rule))
        .collect()
}

fn fingerprints(text: &str) -> Vec<String> {
    check_files(&engine(&["call"]), vec![source("src/a.ts", text)])
        .iter()
        .map(|finding| finding.fingerprint.to_string())
        .collect()
}

#[test]
fn a_rule_sees_only_its_node_types_and_each_file_once() {
    let findings = check_files(
        &engine(&["seen"]),
        vec![
            source("src/a.ts", "f();\nconst x = 1;\ng(x);\n"),
            source("src/b.ts", "const y = 2;\n"),
        ],
    );

    let seen: Vec<String> = findings
        .iter()
        .map(|f| format!("{}:{} {}", f.file, f.start.line, f.message))
        .collect();
    assert_eq!(
        seen,
        [
            "src/a.ts:1 CallExpression",
            "src/a.ts:1 file",
            "src/a.ts:3 CallExpression",
            "src/b.ts:1 file",
        ]
    );
}

#[test]
fn a_disabled_rule_is_silent_and_an_unknown_id_is_an_error() {
    let findings = check_files(&engine(&["call"]), vec![source("src/a.ts", "f();")]);
    assert_eq!(brief(&findings), ["src/a.ts:1:1 call"]);

    let unknown_disabled = RunConfig {
        disabled: vec!["no-such".into()],
        ..RunConfig::default()
    };
    assert_eq!(
        Engine::new(&SET, unknown_disabled).err(),
        Some(ConfigError::UnknownRule {
            key: "disabled[0]".into()
        })
    );

    let unknown_options = RunConfig {
        options: BTreeMap::from([("no-such".into(), toml::Table::new())]),
        ..RunConfig::default()
    };
    assert_eq!(
        Engine::new(&SET, unknown_options).err(),
        Some(ConfigError::UnknownRule {
            key: "rules.no-such".into()
        })
    );
}

#[test]
fn an_opt_in_group_runs_only_when_enabled_names_it() {
    let observe = |config: RunConfig| {
        let engine = Engine::new(&SET, config).unwrap();
        let run = |mode| {
            brief(
                &engine
                    .run(vec![source("src/a.ts", "")], &no_facts(), mode)
                    .findings,
            )
        };
        let active = engine
            .rule_states(&no_facts(), &[])
            .iter()
            .any(|state| state.meta.id == "extra-rule" && state.active);
        (run(Mode::Check), run(Mode::Hook), active)
    };

    assert_eq!(observe(only(&["extra-rule"])), (vec![], vec![], false));

    let found = vec!["src/a.ts:1:1 extra-rule".to_string()];
    let on = RunConfig {
        enabled: vec!["extra".into()],
        ..only(&["extra-rule"])
    };
    assert_eq!(observe(on), (found.clone(), found, true));
}

#[test]
fn disabled_turns_off_a_rule_of_an_enabled_group() {
    let config = RunConfig {
        enabled: vec!["extra".into()],
        ..only(&["per-file"])
    };
    let findings = check_files(
        &Engine::new(&SET, config).unwrap(),
        vec![source("src/a.ts", "")],
    );

    assert_eq!(brief(&findings), ["src/a.ts:1:1 per-file"]);
}

#[test]
fn enabled_names_only_opt_in_groups_and_is_checked_after_disabled() {
    let with_enabled = |names: &[&str]| RunConfig {
        enabled: names.iter().map(|name| name.to_string()).collect(),
        ..RunConfig::default()
    };
    let error = |config| Engine::new(&SET, config).err();

    assert_eq!(
        error(with_enabled(&["extra", "no-such"])),
        Some(ConfigError::UnknownGroup {
            key: "enabled[1]".into()
        })
    );
    assert_eq!(
        error(with_enabled(&["per-file"])),
        Some(ConfigError::UnknownGroup {
            key: "enabled[0]".into()
        }),
        "a rule id is not a group"
    );
    assert_eq!(
        error(with_enabled(&["core"])),
        Some(ConfigError::NotOptIn {
            key: "enabled[0]".into()
        })
    );

    let bad_disabled = RunConfig {
        disabled: vec!["no-such".into()],
        ..with_enabled(&["core"])
    };
    assert_eq!(
        error(bad_disabled),
        Some(ConfigError::UnknownRule {
            key: "disabled[0]".into()
        })
    );
    let bad_rules = RunConfig {
        options: BTreeMap::from([("no-such".into(), toml::Table::new())]),
        ..with_enabled(&["core"])
    };
    assert_eq!(
        error(bad_rules),
        Some(ConfigError::NotOptIn {
            key: "enabled[0]".into()
        }),
        "`enabled` comes before `rules.*`"
    );
}

fn with_depth_options(mut config: RunConfig, options: &str) -> Result<Engine, ConfigError> {
    config
        .options
        .insert("depth".into(), toml::from_str(options).unwrap());
    Engine::new(&SET, config)
}

#[test]
fn config_options_override_rule_options_and_name_the_bad_key() {
    let engine = with_depth_options(only(&["depth"]), "depth = 5").unwrap();
    let findings = check_files(&engine, vec![source("src/a.ts", "")]);
    assert_eq!(findings[0].message, "{depth} deeper than 5");

    assert_eq!(
        with_depth_options(only(&["depth"]), "colour = \"red\"").err(),
        Some(ConfigError::UnknownOption {
            key: "rules.depth.colour".into()
        })
    );
    assert_eq!(
        with_depth_options(only(&[]), "colour = \"red\"").err(),
        Some(ConfigError::UnknownOption {
            key: "rules.depth.colour".into()
        }),
        "keys of a disabled rule are checked too"
    );

    let Some(ConfigError::BadOption { key, message }) =
        with_depth_options(only(&["depth"]), "depth = \"x\"").err()
    else {
        panic!("a string for a u32 option must fail");
    };
    assert_eq!(key, "rules.depth.depth");
    assert!(!message.is_empty());
    assert!(
        matches!(
            with_depth_options(only(&[]), "depth = \"x\"").err(),
            Some(ConfigError::BadOption { .. })
        ),
        "values of a disabled rule are checked too"
    );
}

#[test]
fn a_group_with_requires_package_runs_only_where_the_package_depends_on_it() {
    let engine = engine(&["nest"]);
    let run = |package: Option<Package>| {
        let facts = StaticFacts {
            package,
            ..no_facts()
        };
        brief(
            &engine
                .run(vec![source("src/a.ts", "")], &facts, Mode::Check)
                .findings,
        )
    };

    assert_eq!(run(Some(package("@nestjs/core"))), ["src/a.ts:1:1 nest"]);
    assert!(run(Some(package("express"))).is_empty());
    assert!(run(None).is_empty());
}

#[test]
fn files_picks_tests_or_code_by_the_role_of_the_file() {
    let findings = check_files(
        &engine(&["in-tests", "in-code"]),
        vec![source("src/a.spec.ts", ""), source("src/a.ts", "")],
    );

    assert_eq!(
        brief(&findings),
        ["src/a.spec.ts:1:1 in-tests", "src/a.ts:1:1 in-code"]
    );
}

#[test]
fn hook_runs_only_error_rules() {
    let engine = engine(&["call", "candidate"]);
    let run = |mode| {
        engine
            .run(vec![source("src/a.ts", "f();")], &no_facts(), mode)
            .findings
            .iter()
            .map(|f| (f.rule, f.kind))
            .collect::<Vec<_>>()
    };

    assert_eq!(
        run(Mode::Check),
        [
            ("call", FindingKind::Error),
            ("candidate", FindingKind::Candidate)
        ]
    );
    assert_eq!(run(Mode::Hook), [("call", FindingKind::Error)]);
}

#[test]
fn the_message_fills_args_from_the_report() {
    let findings = check_files(&engine(&["depth"]), vec![source("src/a.ts", "")]);

    assert_eq!(
        findings[0].message, "{depth} deeper than 3",
        "a value is not filled again"
    );
}

#[test]
fn a_file_that_does_not_parse_gets_one_parse_error_and_no_rule_runs() {
    let not_utf8 = SourceFile {
        path: "src/latin1.ts".into(),
        bytes: vec![0xff, 0xfe],
        changed: None,
    };
    let findings = check_files(
        &engine(&["per-file"]),
        vec![
            source("src/broken.ts", "let = ;"),
            not_utf8,
            source("src/notes.txt", "f();"),
        ],
    );

    assert_eq!(
        brief(&findings),
        [
            "src/broken.ts:1:7 parse-error",
            "src/latin1.ts:1:1 parse-error",
            "src/notes.txt:1:1 parse-error",
        ]
    );
    assert!(
        findings
            .iter()
            .all(|f| f.kind == FindingKind::ParseError && !f.message.is_empty())
    );
    assert_eq!(
        findings[1].message,
        "the file is not UTF-8, so no rule checked it"
    );
}

#[test]
fn positions_skip_the_bom_and_count_crlf_lines() {
    let mut with_bom = source("src/bom.ts", "f();");
    with_bom.bytes.splice(0..0, [0xEF, 0xBB, 0xBF]);
    let findings = check_files(
        &engine(&["call"]),
        vec![with_bom, source("src/crlf.ts", "a;\r\nf();\r\n")],
    );

    let spans: Vec<_> = findings
        .iter()
        .map(|f| (f.start.line, f.start.column, f.end.line, f.end.column))
        .collect();
    assert_eq!(spans, [(1, 1, 1, 4), (2, 1, 2, 4)]);
}

#[test]
fn in_hunk_checks_the_first_line_of_the_finding() {
    let engine = engine(&["call"]);
    // `f()` on line 1; `g(\n1)` on lines 2-3.
    let in_hunk = |changed: Option<Changed>| {
        let file = SourceFile {
            changed,
            ..source("src/a.ts", "f();\ng(\n1);\n")
        };
        check_files(&engine, vec![file])
            .iter()
            .map(|f| f.in_hunk)
            .collect::<Vec<_>>()
    };
    let lines = |start, end| {
        Some(Changed {
            lines: vec![LineRange { start, end }],
            ..Changed::default()
        })
    };

    assert_eq!(in_hunk(None), [None, None]);
    assert_eq!(in_hunk(lines(1, 1)), [Some(true), Some(false)]);
    assert_eq!(in_hunk(lines(2, 2)), [Some(false), Some(true)]);
    assert_eq!(
        in_hunk(lines(3, 3)),
        [Some(false), Some(false)],
        "only a later line of `g` changed"
    );
}

#[test]
fn a_finding_names_the_function_member_or_statement_around_it() {
    let text = "\
f();
function named(x = f()) {
  f();
}
const arrow = () => f();
class Orders {
  field = f();
  #secret = () => f();
  @dec
  method() {
    items.map(() => f());
  }
  static [key()]() {}
}
const Anon = class {
  run() { f(); }
};
const o = { key() { f(); }, value: (() => f()) as Fn };
exports.handler = function () { f(); };
export default function () { f(); }
export const { a, b } = f();
";
    let findings = check_files(&engine(&["call"]), vec![source("src/a.ts", text)]);
    let functions: Vec<(u32, &str)> = findings
        .iter()
        .map(|f| (f.start.line, f.function.as_deref().unwrap()))
        .collect();

    assert_eq!(
        functions,
        [
            (1, "(top level)"),
            (2, "named"),
            (3, "named"),
            (5, "arrow"),
            (7, "Orders.field"),
            (8, "Orders.#secret"),
            (11, "Orders.method"),
            (11, "(anonymous)"),
            (13, "Orders.[key()]"),
            (16, "Anon.run"),
            (18, "key"),
            (18, "value"),
            (19, "exports.handler"),
            (20, "(anonymous)"),
            (21, "{ a, b }"),
        ]
    );
}

#[test]
fn a_finding_in_a_comment_belongs_to_the_code_the_comment_is_about() {
    let text = "\
/** Totals. */
class Orders {
  /** Sums. */
  @dec
  sum() {
    return 1; // why
  }
  // loose
}
f(); // after a call
// at the end
";
    let findings = check_files(&engine(&["comment"]), vec![source("src/a.ts", text)]);
    let functions: Vec<(u32, &str)> = findings
        .iter()
        .map(|f| (f.start.line, f.function.as_deref().unwrap()))
        .collect();

    assert_eq!(
        functions,
        [
            (1, "Orders"),
            (3, "Orders.sum"),
            (6, "Orders.sum"),
            (8, "Orders"),
            (10, "(top level)"),
            (11, "(top level)"),
        ]
    );
}

#[test]
fn a_finding_about_the_file_has_no_function() {
    let findings = check_files(
        &engine(&["on-path"]),
        vec![source("src/a.ts", "f();"), source("src/b.ts", "let = ;")],
    );

    assert_eq!(
        brief(&findings),
        ["src/a.ts:1:1 on-path", "src/b.ts:1:7 parse-error"]
    );
    assert!(findings.iter().all(|f| f.function.is_none()));
}

/// `(x, y, z)`: whether the diff touched the function of the call on each of them.
fn touched(changed: Option<Changed>) -> Vec<Option<bool>> {
    let text = "\
function a() {
  /* one
     two */
  x();
}
function b() {
  y();

  z();
}
";
    let file = SourceFile {
        changed,
        ..source("src/a.ts", text)
    };
    check_files(&engine(&["call"]), vec![file])
        .iter()
        .map(|f| f.touched)
        .collect()
}

fn diff(lines: &[(u32, u32)], cuts: &[(u32, u32)]) -> Option<Changed> {
    Some(Changed {
        lines: lines
            .iter()
            .map(|&(start, end)| LineRange { start, end })
            .collect(),
        cuts: cuts
            .iter()
            .map(|&(start, count)| Cut { start, count })
            .collect(),
        new_path: false,
    })
}

#[test]
fn touched_counts_a_changed_line_of_code_of_the_function() {
    let (yes, no) = (Some(true), Some(false));

    assert_eq!(touched(None), [None, None, None]);
    assert_eq!(touched(diff(&[(4, 4)], &[])), [yes, no, no]);
    assert_eq!(
        touched(diff(&[(5, 6)], &[])),
        [yes, yes, yes],
        "the braces are code"
    );
    assert_eq!(touched(diff(&[(1, u32::MAX)], &[])), [yes, yes, yes]);
    assert_eq!(
        touched(diff(&[(2, 3), (8, 8)], &[])),
        [no, no, no],
        "a comment and a blank line are not code"
    );
}

#[test]
fn touched_counts_code_the_diff_took_out_of_the_function() {
    let (yes, no) = (Some(true), Some(false));

    assert_eq!(touched(diff(&[], &[(7, 0)])), [no, yes, yes]);
    assert_eq!(
        touched(diff(&[], &[(5, 0)])),
        [no, no, no],
        "between the functions"
    );
    assert_eq!(
        touched(diff(&[(2, 3)], &[(2, 2)])),
        [yes, no, no],
        "code turned into a comment"
    );
}

#[test]
fn a_finding_about_the_file_is_touched_when_the_diff_adds_or_renames_it() {
    let touched = |changed: Option<Changed>| {
        let file = SourceFile {
            changed,
            ..source("src/a.ts", "f();\n")
        };
        check_files(&engine(&["on-path"]), vec![file])[0].touched
    };
    let new_path = |new_path| {
        Some(Changed {
            lines: vec![LineRange { start: 1, end: 1 }],
            new_path,
            ..Changed::default()
        })
    };

    assert_eq!(touched(None), None);
    assert_eq!(touched(new_path(true)), Some(true));
    assert_eq!(
        touched(new_path(false)),
        Some(false),
        "an edit of its lines is not enough"
    );
}

#[test]
fn a_fingerprint_follows_the_text_and_the_function_around_not_the_line() {
    let base = fingerprints("function f() { g(); }");

    assert_eq!(
        fingerprints("\n\nconst x = 1;\nfunction f() { g(); }"),
        base
    );
    assert_ne!(fingerprints("function h() { g(); }"), base);
    assert_ne!(fingerprints("g();"), base);
    assert_eq!(
        fingerprints("function f() { g(); g(); }"),
        [base[0].clone(), base[0].clone()]
    );
    for named_f in [
        "const f = () => { g(); };",
        "const f = function () { g(); };",
        "const x = function f() { g(); };",
        "class C { f() { g(); } }",
        "class C { f = () => { g(); }; }",
        "const o = { f() { g(); } };",
        "const o = { f: () => { g(); } };",
        "function f() { return function () { g(); }; }",
    ] {
        assert_eq!(fingerprints(named_f), base, "{named_f}");
    }
}

#[test]
fn the_fingerprint_hash_stays_fixed() {
    assert_eq!(fingerprints("function f() { g(); }"), ["call:2d98cf"]);
}

#[test]
fn an_ignored_pair_hides_the_finding_only_in_its_file() {
    let ignored = (
        Utf8PathBuf::from("src/a.ts"),
        fingerprints("g();")[0].clone(),
    );
    let config = RunConfig {
        ignored: BTreeSet::from([ignored]),
        ..only(&["call"])
    };
    let findings = check_files(
        &Engine::new(&SET, config).unwrap(),
        vec![source("src/a.ts", "g();"), source("src/b.ts", "g();")],
    );

    assert_eq!(brief(&findings), ["src/b.ts:1:1 call"]);
}

#[test]
fn findings_are_sorted_by_file_line_column_and_rule_on_any_thread_count() {
    let engine = engine(&["call", "per-file"]);
    let files = || -> Vec<SourceFile> {
        (0..20)
            .rev()
            .map(|i| source(&format!("src/f{i:02}.ts"), "g(h());\nf();\n"))
            .collect()
    };
    let on_threads = |threads| {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| brief(&check_files(&engine, files())))
    };

    let expected: Vec<String> = (0..20)
        .flat_map(|i| {
            ["1:1 call", "1:1 per-file", "1:3 call", "2:1 call"]
                .map(|place| format!("src/f{i:02}.ts:{place}"))
        })
        .collect();
    assert_eq!(on_threads(1), expected);
    assert_eq!(on_threads(4), expected);
}

#[test]
fn facts_needed_takes_uses_of_enabled_rules_and_packages_for_group_conditions() {
    let all = engine(&["call", "candidate", "nest"]);
    assert_eq!(
        all.facts_needed(Mode::Check),
        [Fact::Packages, Fact::Aliases, Fact::Enums]
    );
    assert_eq!(
        all.facts_needed(Mode::Hook),
        [Fact::Packages, Fact::Aliases],
        "no facts of candidates"
    );

    let without_nest = engine(&["call", "candidate"]);
    assert_eq!(without_nest.facts_needed(Mode::Check), [Fact::Enums]);
    assert_eq!(without_nest.facts_needed(Mode::Hook), []);
}

/// The package of one file only, as in a monorepo.
struct OnePackage {
    file: Utf8PathBuf,
    package: Package,
    enums: EnumIndex,
}

impl Facts for OnePackage {
    fn package(&self, file: &Utf8Path) -> Option<&Package> {
        (file == self.file).then_some(&self.package)
    }

    fn aliases(&self, _file: &Utf8Path) -> &[String] {
        &[]
    }

    fn enums(&self) -> &EnumIndex {
        &self.enums
    }
}

#[test]
fn rule_states_lists_every_rule_with_disabled_and_group_conditions() {
    let engine = engine(&["call", "nest", "agent"]);
    let facts = OnePackage {
        file: "apps/api/main.ts".into(),
        package: package("@nestjs/core"),
        enums: EnumIndex::new(Vec::new()),
    };
    let states = |files: &[&str]| -> BTreeMap<&str, bool> {
        let files: Vec<Utf8PathBuf> = files.iter().map(Utf8PathBuf::from).collect();
        let states = engine.rule_states(&facts, &files);
        assert_eq!(states.len(), SET.rules.len());
        states
            .iter()
            .map(|state| (state.meta.id, state.active))
            .collect()
    };

    let both = states(&["apps/web/main.ts", "apps/api/main.ts"]);
    assert!(both["call"]);
    assert!(!both["per-file"], "disabled");
    assert!(both["nest"], "the condition holds for one of the files");
    assert!(both["agent"]);

    let web = states(&["apps/web/main.ts"]);
    assert!(!web["nest"], "the condition holds for no file");
}

#[test]
fn fixable_needs_fix_in_rule_toml_and_an_edit() {
    let findings = check_files(
        &engine(&["fix-on", "call"]),
        vec![source("src/a.ts", "edit();\nkeep();\n")],
    );

    let fixable: Vec<_> = findings
        .iter()
        .map(|f| (f.start.line, f.rule, f.fixable))
        .collect();
    assert_eq!(
        fixable,
        [
            (1, "call", false),
            (1, "fix-on", true),
            (2, "call", false),
            (2, "fix-on", false),
        ]
    );
}

fn fix_files(engine: &Engine, files: Vec<SourceFile>) -> RunReport {
    engine.run(files, &no_facts(), Mode::Fix)
}

fn rewritten(report: &RunReport) -> Vec<String> {
    report
        .rewritten
        .iter()
        .map(|file| format!("{}: {}", file.path, String::from_utf8_lossy(&file.bytes)))
        .collect()
}

/// What `Fix` writes to `src/a.ts` (`None`: the file stays as it is) and the `fixed` count
/// of each rule there.
fn fix(engine: &Engine, text: &str) -> (Option<String>, Vec<(&'static str, u32)>) {
    let report = fix_files(engine, vec![source("src/a.ts", text)]);
    let text = report
        .rewritten
        .into_iter()
        .next()
        .map(|file| String::from_utf8(file.bytes).unwrap());
    let counts = report.fixed.iter().map(|f| (f.rule, f.count)).collect();
    (text, counts)
}

fn count(file: &str, rule: &'static str, count: u32) -> FixCount {
    FixCount {
        rule,
        file: file.into(),
        count,
    }
}

#[test]
fn fix_applies_edits_that_do_not_touch_in_at_most_three_passes() {
    let engine = engine(&["unwrap"]);

    assert_eq!(
        fix(&engine, "wrap(1); wrap(2);"),
        (Some("1; 2;".into()), vec![("unwrap", 2)])
    );
    assert_eq!(
        fix(&engine, "wrap(wrap(1));"),
        (Some("1;".into()), vec![("unwrap", 2)]),
        "the inner edit lies inside the outer one and waits for the next pass"
    );
    assert_eq!(
        fix(&engine, "wrap(wrap(wrap(wrap(1))));"),
        (Some("wrap(1);".into()), vec![("unwrap", 3)])
    );
}

#[test]
fn a_second_fix_of_the_fixed_text_changes_nothing() {
    let engine = engine(&["unwrap"]);
    let first = fix_files(&engine, vec![source("src/a.ts", "wrap(wrap(1));")]);
    assert_eq!(rewritten(&first), ["src/a.ts: 1;"]);

    let fixed = SourceFile {
        bytes: first.rewritten.into_iter().next().unwrap().bytes,
        ..source("src/a.ts", "")
    };
    let again = fix_files(&engine, vec![fixed]);

    assert!(again.rewritten.is_empty());
    assert!(again.fixed.is_empty());
}

#[test]
fn an_edit_that_breaks_parsing_leaves_its_file_and_edits_of_fix_false_rules_are_dropped() {
    let report = fix_files(
        &engine(&["unwrap", "break"]),
        vec![
            source("src/a.ts", "wrap(1); breaks();"),
            source("src/b.ts", "wrap(2);"),
        ],
    );
    assert_eq!(
        rewritten(&report),
        ["src/b.ts: 2;"],
        "the good edit of `src/a.ts` is dropped too"
    );
    assert_eq!(report.fixed, [count("src/b.ts", "unwrap", 1)]);

    assert_eq!(fix(&engine(&["call"]), "edit();"), (None, vec![]));
    assert_eq!(
        fix(&engine(&["fix-on"]), "edit();"),
        (Some(";".into()), vec![("fix-on", 1)])
    );
}

#[test]
fn fixed_counts_fixed_findings_by_file_and_rule() {
    let report = fix_files(
        &engine(&["unwrap", "fix-on"]),
        vec![
            source("src/b.ts", "wrap(3);"),
            source("src/a.ts", "wrap(1); wrap(2); edit();"),
        ],
    );

    assert_eq!(
        report.fixed,
        [
            count("src/a.ts", "fix-on", 1),
            count("src/a.ts", "unwrap", 2),
            count("src/b.ts", "unwrap", 1),
        ]
    );
    assert_eq!(rewritten(&report), ["src/a.ts: 1; 2; ;", "src/b.ts: 3;"]);
}

#[test]
fn fix_keeps_the_bom_and_writes_the_line_ending_of_the_file() {
    let mut with_bom = source("src/bom.ts", "wrap(1);");
    with_bom.bytes.splice(0..0, [0xEF, 0xBB, 0xBF]);
    let report = fix_files(&engine(&["unwrap"]), vec![with_bom]);
    assert_eq!(report.rewritten[0].bytes, b"\xEF\xBB\xBF1;");

    let split = engine(&["split"]);
    assert_eq!(
        fix(&split, "x;\r\npair();\r\n").0.unwrap(),
        "x;\r\na();\r\nb();\r\n"
    );
    assert_eq!(fix(&split, "pair();\n").0.unwrap(), "a();\nb();\n");

    let crlf_edit = RunConfig {
        options: BTreeMap::from([(
            "split".into(),
            toml::from_str(r#"ending = "\r\n""#).unwrap(),
        )]),
        ..only(&["split"])
    };
    assert_eq!(
        fix(&Engine::new(&SET, crlf_edit).unwrap(), "x;\r\npair();\r\n")
            .0
            .unwrap(),
        "x;\r\na();\r\nb();\r\n",
        "a `\r\n` of the rule is not doubled"
    );
}

#[test]
fn fix_leaves_an_ignored_finding_as_it_is() {
    let text = "function f() { wrap(1); }\nfunction g() { wrap(1); }";
    let in_f = check_files(&engine(&["unwrap"]), vec![source("src/a.ts", text)])[0]
        .fingerprint
        .to_string();
    let config = RunConfig {
        ignored: BTreeSet::from([("src/a.ts".into(), in_f)]),
        ..only(&["unwrap"])
    };

    assert_eq!(
        fix(&Engine::new(&SET, config).unwrap(), text),
        (
            Some("function f() { wrap(1); }\nfunction g() { 1; }".into()),
            vec![("unwrap", 1)]
        )
    );
}

#[test]
fn fix_runs_only_fix_rules_and_reports_no_findings() {
    let engine = engine(&["unwrap", "per-file", "candidate"]);
    let run = |mode| engine.run(vec![source("src/a.ts", "wrap(1);")], &no_facts(), mode);

    let fixed = run(Mode::Fix);
    assert!(fixed.findings.is_empty());
    assert_eq!(fixed.fixed, [count("src/a.ts", "unwrap", 1)]);
    assert_eq!(
        engine.facts_needed(Mode::Fix),
        [],
        "no facts of `candidate`"
    );

    for mode in [Mode::Check, Mode::Hook] {
        let report = run(mode);
        assert!(!report.findings.is_empty(), "{mode:?}");
        assert!(report.fixed.is_empty(), "{mode:?}");
        assert!(report.rewritten.is_empty(), "{mode:?}");
    }
}

#[test]
fn broken_edits_are_not_applied_and_do_not_stop_the_run() {
    let text = "wrap(1); const s = \"é\";";
    assert_eq!(
        fix(&engine(&["bad-edits", "unwrap"]), text),
        (Some("1; const s = \"é\";".into()), vec![("unwrap", 1)])
    );

    let findings = check_files(&engine(&["bad-edits"]), vec![source("src/a.ts", text)]);
    assert_eq!(findings.len(), 4);
    assert!(
        findings.iter().all(|f| !f.fixable),
        "`Fix` never takes them"
    );
}

#[test]
fn an_edit_that_changes_nothing_is_no_fix() {
    let engine = engine(&["same", "unwrap"]);
    assert_eq!(
        fix(&engine, "wrap(1);"),
        (Some("1;".into()), vec![("unwrap", 1)]),
        "taken first in every pass, the edit of `same` would hold back `unwrap`"
    );

    let fixable: Vec<_> = check_files(&engine, vec![source("src/a.ts", "wrap(1);")])
        .iter()
        .map(|f| (f.rule, f.fixable))
        .collect();
    assert_eq!(fixable, [("same", false), ("unwrap", true)]);
}

#[test]
fn functions_gives_each_function_with_a_body_and_where_v8_may_start_it() {
    let text = "export function total(xs: number[]) {\n  return xs.map((x) => x * 2);\n}\n\
                const label = () => \"a\";\n\
                class Report {\n  build() {\n    function inner() {}\n    return inner;\n  }\n  static async load() {}\n}\n\
                const e = \"é😀\", f = () => 1;\n\
                run(function () {});\n\
                declare function gone(): void;\n";

    let spans = Engine::functions(&source("src/a.ts", text));
    let seen: Vec<String> = spans
        .iter()
        .map(|span| {
            let (start, end) = (&span.start, &span.end);
            let name = span.name.as_deref().unwrap_or("-");
            format!(
                "{name} {}:{}-{}:{}",
                start.line, start.column, end.line, end.column
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            // From `export`, where coverage starts it.
            "total 1:1-3:2",
            // An anonymous callback takes the name of the function around it.
            "total 2:17-2:29",
            "label 4:15-4:24",
            // From the name of the method.
            "build 6:3-9:4",
            "inner 7:5-7:24",
            // From `static`: coverage starts it at `async`.
            "load 10:3-10:25",
            // Columns count characters: `é` is 2 bytes, the emoji 4 bytes and 2 UTF-16 units.
            "f 12:21-12:28",
            "- 13:5-13:19",
        ]
    );
    assert!(Engine::functions(&source("src/b.ts", "function (")).is_empty());
}

#[test]
fn a_report_span_past_the_text_or_inside_a_char_is_cut_to_fit() {
    let engine = engine(&["bad-spans"]);
    let text = "const s = \"é\";";

    let findings = check_files(&engine, vec![source("src/a.ts", text)]);
    assert_eq!(
        brief(&findings),
        [
            "src/a.ts:1:2 bad-spans",
            "src/a.ts:1:12 bad-spans",
            "src/a.ts:1:15 bad-spans",
        ]
    );
    assert_eq!(
        fix(&engine, text),
        (Some(format!("0;{text}")), vec![("bad-spans", 1)])
    );
}
