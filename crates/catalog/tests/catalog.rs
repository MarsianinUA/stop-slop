use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use catalog::{Expected, FolderError, Problem, generate, read_fixture, write_if_changed};

const ERROR_RULE: &str = "description = \"d\"
kind = \"error\"
source = [\"personal\"]
message = \"m\"
";

const CANDIDATE_RULE: &str = "description = \"d\"
kind = \"candidate\"
source = [\"personal\"]
message = \"m\"
";

const AGENT_RULE: &str = "description = \"d\"
kind = \"agent\"
source = [\"personal\"]
message = \"m\"
agent = \"\"\"
first line
second line
\"\"\"
";

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A complete `error` or `candidate` rule folder `<group>/<id>` with the given `rule.toml`.
fn check_rule(root: &Path, folder: &str, toml: &str) {
    let id = folder.rsplit('/').next().unwrap();
    write(root, &format!("{folder}/rule.toml"), toml);
    write(root, &format!("{folder}/check.rs"), "");
    write(root, &format!("{folder}/valid.ts"), "let a = 1;\n");
    let invalid = format!("let b = 1; // expect: {id}\n");
    write(root, &format!("{folder}/invalid.ts"), &invalid);
}

fn folder_errors(root: &Path) -> Vec<FolderError> {
    let Err(errors) = generate(root) else {
        panic!("expected folder errors");
    };
    errors
}

fn problems(root: &Path) -> Vec<(Option<String>, Problem)> {
    folder_errors(root)
        .into_iter()
        .map(|e| (e.rule, e.problem))
        .collect()
}

fn rule(id: &str) -> Option<String> {
    Some(id.to_string())
}

/// The `RuleMeta` literal of rule `id` in the generated code.
fn meta<'c>(code: &'c str, id: &str) -> &'c str {
    let start = code.find(&format!("id: {id:?}")).unwrap();
    let end = start + code[start..].find("imp:").unwrap();
    &code[start..end]
}

#[test]
fn three_rules_become_three_wrappers_with_slash_paths_and_kit_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    check_rule(root, "types/no-any", ERROR_RULE);
    let candidate = format!("{CANDIDATE_RULE}\n[options]\nmax = 4\n");
    check_rule(root, "types/max-depth", &candidate);
    write(
        root,
        "types/group.toml",
        "requires-package = \"@acme/core\"\n",
    );
    let agent = AGENT_RULE.replace('\n', "\r\n");
    write(
        root,
        "functions-classes/one-responsibility/rule.toml",
        &agent,
    );
    write(
        root,
        "roles.toml",
        "[tests]\nsuffixes = [\"spec\"]\ndirs = [\"__tests__\"]\n",
    );

    let generated = generate(root).unwrap();
    let code = generated.code;

    let slash = root.to_str().unwrap().replace('\\', "/");
    for (id, module) in [("no-any", "no_any"), ("max-depth", "max_depth")] {
        let wrapper =
            format!("#[path = \"{slash}/types/{id}\"]\nmod {module} {{\n    mod check;\n");
        assert!(code.contains(&wrapper), "{wrapper}\n---\n{code}");
    }
    assert!(code.contains("mod one_responsibility {\n    pub(super) const DEF"));
    assert_eq!(
        code.matches("pub(super) const DEF: ::kit::RuleDef").count(),
        3
    );
    assert_eq!(code.matches("#[path").count(), 2);
    assert!(!code.contains(r"\\?\"), "{code}");
    assert!(!code.replace("::kit::", "").contains("kit::"), "{code}");
    assert!(!code.contains("\\r"), "{code}");

    assert!(code.contains("imp: ::kit::Impl::Agent"));
    assert!(code.contains("agent: Some(\"first line\\nsecond line\\n\")"));
    assert!(meta(&code, "max-depth").contains("kind: ::kit::RuleKind::Candidate"));
    assert!(meta(&code, "max-depth").contains("options: \"max = 4\\n\""));
    assert!(meta(&code, "no-any").contains("options: \"\""));
    assert!(code.contains("rules: &[one_responsibility::DEF, max_depth::DEF, no_any::DEF]"));
    assert!(
        code.contains("::kit::GroupDef { name: \"functions-classes\", requires_package: None }")
    );
    assert!(
        code.contains(
            "::kit::GroupDef { name: \"types\", requires_package: Some(\"@acme/core\") }"
        )
    );
    assert!(
        code.contains("tests: ::kit::RoleDef { suffixes: &[\"spec\"], dirs: &[\"__tests__\"] }")
    );
    assert!(code.contains("types: ::kit::RoleDef { suffixes: &[], dirs: &[] }"));
    assert_eq!(
        generated.watch,
        [
            root.to_path_buf(),
            root.join("functions-classes"),
            root.join("types")
        ]
    );
}

#[test]
fn broken_folders_report_every_error_with_the_rule_id() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    check_rule(
        root,
        "types/colour",
        &format!("{ERROR_RULE}colour = \"red\"\n"),
    );
    write(root, "types/coded/rule.toml", AGENT_RULE);
    write(root, "types/coded/check.rs", "");
    write(root, "naming/bare/rule.toml", ERROR_RULE);
    write(root, "naming/bare/valid.ts", "");
    write(root, "naming/bare/invalid.ts", "let a; // expect: bare\n");

    let errors = folder_errors(root);

    let found: Vec<_> = errors
        .iter()
        .map(|e| (e.rule.clone(), &e.problem))
        .collect();
    assert_eq!(
        found,
        [
            (rule("bare"), &Problem::MissingCheck),
            (rule("coded"), &Problem::AgentHasCode),
            (rule("colour"), &Problem::UnknownKey("colour".to_string())),
        ]
    );
    let text = errors[0].to_string();
    assert!(text.contains("bare") && text.contains("naming"), "{text}");
}

#[test]
fn unknown_keys_in_rule_group_and_roles_toml_are_named() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    check_rule(root, "types/a", &format!("{ERROR_RULE}colour = \"red\"\n"));
    write(root, "types/group.toml", "size = 1\n");
    write(root, "roles.toml", "[tests]\nglobs = []\n");

    assert_eq!(
        problems(root),
        [
            (None, Problem::UnknownKey("globs".to_string())),
            (None, Problem::UnknownKey("size".to_string())),
            (rule("a"), Problem::UnknownKey("colour".to_string())),
        ]
    );
}

#[test]
fn ids_must_be_kebab_case_unique_and_not_rust_keywords() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for folder in [
        "naming/dup",
        "types/dup",
        "types/Bad_Id",
        "types/type",
        "types/gen",
    ] {
        check_rule(root, folder, ERROR_RULE);
    }

    assert_eq!(
        problems(root),
        [
            (rule("Bad_Id"), Problem::BadId),
            (rule("dup"), Problem::DuplicateId),
            (rule("gen"), Problem::RustKeyword),
            (rule("type"), Problem::RustKeyword),
        ]
    );
}

#[test]
fn an_agent_rule_has_only_a_rule_toml_with_a_hint() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "agents/with-code/rule.toml", AGENT_RULE);
    write(root, "agents/with-code/check.rs", "");
    write(root, "agents/with-fixtures/rule.toml", AGENT_RULE);
    write(root, "agents/with-fixtures/valid.ts", "");
    write(
        root,
        "agents/with-fix/rule.toml",
        &format!("fix = true\n{AGENT_RULE}"),
    );
    let no_hint = &AGENT_RULE[..AGENT_RULE.find("agent =").unwrap()];
    write(root, "agents/no-hint/rule.toml", no_hint);
    write(
        root,
        "agents/empty-hint/rule.toml",
        &format!("{no_hint}agent = \" \"\n"),
    );

    assert_eq!(
        problems(root),
        [
            (rule("empty-hint"), Problem::AgentNoHint),
            (rule("no-hint"), Problem::AgentNoHint),
            (rule("with-code"), Problem::AgentHasCode),
            (rule("with-fix"), Problem::AgentHasFix),
            (rule("with-fixtures"), Problem::AgentHasFixtures),
        ]
    );
}

#[test]
fn error_and_candidate_rules_need_check_rs_and_both_fixtures() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    check_rule(root, "types/no-check", ERROR_RULE);
    fs::remove_file(root.join("types/no-check/check.rs")).unwrap();
    check_rule(root, "types/no-valid", CANDIDATE_RULE);
    fs::remove_file(root.join("types/no-valid/valid.ts")).unwrap();
    check_rule(root, "types/no-invalid", ERROR_RULE);
    fs::remove_file(root.join("types/no-invalid/invalid.ts")).unwrap();

    assert_eq!(
        problems(root),
        [
            (rule("no-check"), Problem::MissingCheck),
            (rule("no-invalid"), Problem::MissingFixtures),
            (rule("no-valid"), Problem::MissingFixtures),
        ]
    );
}

#[test]
fn fixed_ts_exists_exactly_when_fix_is_true() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    check_rule(
        root,
        "imports/fix-only",
        &format!("fix = true\n{ERROR_RULE}"),
    );
    check_rule(root, "imports/fixed-only", ERROR_RULE);
    write(root, "imports/fixed-only/fixed.ts", "");

    assert_eq!(
        problems(root),
        [
            (rule("fix-only"), Problem::FixMismatch),
            (rule("fixed-only"), Problem::FixMismatch),
        ]
    );
}

#[test]
fn every_invalid_fixture_needs_marks_with_its_own_id() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    check_rule(root, "types/unmarked", ERROR_RULE);
    write(root, "types/unmarked/invalid.ts", "let b = 1;\n");
    check_rule(root, "types/foreign", ERROR_RULE);
    write(
        root,
        "types/foreign/invalid.other.ts",
        "let c; // expect: no-any\n",
    );

    assert_eq!(
        problems(root),
        [
            (
                rule("foreign"),
                Problem::ForeignExpect("no-any".to_string())
            ),
            (rule("unmarked"), Problem::NoExpect),
        ]
    );
}

#[test]
fn extra_files_and_unknown_sources_are_errors() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "README.md", "files in the root are skipped");
    check_rule(root, "types/a", ERROR_RULE);
    write(root, "types/a/notes.md", "");
    write(root, "types/a/sub/x.ts", "");
    let blog = ERROR_RULE.replace("[\"personal\"]", "[\"personal\", \"blog\"]");
    check_rule(root, "types/b", &blog);
    write(root, "types/readme.md", "");

    assert_eq!(
        problems(root),
        [
            (None, Problem::ExtraFile("readme.md".to_string())),
            (rule("a"), Problem::ExtraFile("notes.md".to_string())),
            (rule("a"), Problem::ExtraFile("sub".to_string())),
            (rule("b"), Problem::UnknownSource("blog".to_string())),
        ]
    );
}

#[test]
fn files_comes_from_the_rule_then_the_group_then_all() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "tests/group.toml", "files = \"tests\"\n");
    check_rule(root, "tests/inherits", ERROR_RULE);
    check_rule(
        root,
        "tests/own",
        &format!("files = \"code\"\n{ERROR_RULE}"),
    );
    check_rule(root, "types/plain", ERROR_RULE);

    let code = generate(root).unwrap().code;

    assert!(meta(&code, "inherits").contains("files: ::kit::Files::Tests"));
    assert!(meta(&code, "own").contains("files: ::kit::Files::Code"));
    assert!(meta(&code, "plain").contains("files: ::kit::Files::All"));
}

#[test]
fn read_fixture_takes_headers_and_marks_and_keeps_line_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("invalid.ts");
    fs::write(
        &file,
        "// @path src/orders/order.response.ts
// @alias @/*
// @workspace @acme/x
// @dependency events
// @dependency buffer
// @ts-nocheck
let a: any; // expect: no-any
let b: any = 1 as any;   // expect: no-any x2
// @path not-a-header
",
    )
    .unwrap();

    let fixture = read_fixture(&file).unwrap();

    assert_eq!(fixture.path, "src/orders/order.response.ts");
    assert_eq!(fixture.aliases, ["@/*"]);
    assert_eq!(fixture.workspace, ["@acme/x"]);
    assert_eq!(fixture.dependencies, ["events", "buffer"]);
    assert_eq!(
        fixture.text,
        "\n\n\n\n\n// @ts-nocheck\nlet a: any;\nlet b: any = 1 as any;\n// @path not-a-header\n"
    );
    let expected = |line, count| Expected {
        line,
        rule: "no-any".to_string(),
        count,
    };
    assert_eq!(fixture.expected, [expected(7, 1), expected(8, 2)]);

    fs::write(&file, "let a; // expect: no-any\r\nlet b;\r\n").unwrap();
    let crlf = read_fixture(&file).unwrap();
    assert_eq!(crlf.path, "src/fixture.ts");
    assert_eq!(crlf.text, "let a;\r\nlet b;\r\n");

    let bom_and_string =
        "\u{feff}// @path src/a.service.ts\nlet s = '// expect: x'; // expect: no-any\n";
    fs::write(&file, bom_and_string).unwrap();
    let marked = read_fixture(&file).unwrap();
    assert_eq!(marked.path, "src/a.service.ts");
    assert_eq!(marked.text, "\nlet s = '// expect: x';\n");
    assert_eq!(marked.expected, [expected(2, 1)]);
}

#[test]
fn write_if_changed_leaves_the_same_text_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.rs");
    fs::write(&path, "code").unwrap();
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(86_400);
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(old)
        .unwrap();

    write_if_changed(&path, "code").unwrap();
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), old);

    write_if_changed(&path, "new code").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "new code");
}

#[test]
fn bad_marks_and_unreadable_fixtures_name_the_rule() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    check_rule(root, "types/zero", ERROR_RULE);
    write(root, "types/zero/invalid.ts", "let a; // expect: zero x0\n");
    check_rule(root, "types/binary", ERROR_RULE);
    fs::write(root.join("types/binary/invalid.ts"), [0xff, 0xfe, b'\n']).unwrap();

    let found = problems(root);
    assert!(
        matches!(found.as_slice(), [
            (Some(binary), Problem::Io(_)),
            (Some(zero), Problem::BadFixture(_)),
        ] if binary == "binary" && zero == "zero"),
        "{found:?}"
    );

    let file = root.join("bad.ts");
    for text in [
        "let a; // expect:\n",
        "let a; // expect: no-any xабв\n",
        "// @path\nlet a;\n",
        "// @path src/a.ts\n// @path src/b.ts\nlet a;\n",
        "// @dependency events util\nlet a;\n",
    ] {
        fs::write(&file, text).unwrap();
        let error = read_fixture(&file).unwrap_err();
        assert!(matches!(error.problem, Problem::BadFixture(_)), "{text}");
    }
}
