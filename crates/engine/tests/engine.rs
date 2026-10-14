use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};
use engine::{ConfigError, Engine, Finding, LineRange, Mode, RunConfig, SourceFile};
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
        RuleDef {
            meta: RuleMeta {
                id: "agent",
                kind: RuleKind::Agent,
                agent: Some("look"),
                ..META
            },
            imp: Impl::Agent,
        },
    ],
    groups: &[
        GroupDef {
            name: "core",
            requires_package: None,
        },
        GroupDef {
            name: "nestjs",
            requires_package: Some("@nestjs/core"),
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
    let in_hunk = |changed: Option<Vec<LineRange>>| {
        let file = SourceFile {
            changed,
            ..source("src/a.ts", "f();\ng(\n1);\n")
        };
        check_files(&engine, vec![file])
            .iter()
            .map(|f| f.in_hunk)
            .collect::<Vec<_>>()
    };
    let lines = |start, end| Some(vec![LineRange { start, end }]);

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
