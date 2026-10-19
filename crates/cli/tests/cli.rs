use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use kit::RuleMeta;
use rules::RULESET;
use serde_json::{Value, json};
use state::{IgnoreEntry, State};
use testkit::TempRepo;

const EXE: &str = env!("CARGO_BIN_EXE_stop-slop");

fn exe(dir: &Path) -> Command {
    let mut command = Command::new(EXE);
    command.current_dir(dir).stdin(Stdio::null());
    command
}

fn run(dir: &Path, args: &[&str]) -> Output {
    exe(dir).args(args).output().expect("start stop-slop")
}

/// Git stops at the parent of the temp folder `dir`, so a repo above it (dotfiles in the home
/// folder) is not found. `repo` passes the variable on to git.
fn exe_outside(dir: &Path) -> Command {
    let parent = dir.parent().expect("a temp folder has a parent");
    let mut command = exe(dir);
    command.env("GIT_CEILING_DIRECTORIES", parent);
    command
}

fn run_outside(args: &[&str]) -> Output {
    let dir = tempfile::tempdir().expect("create a temp folder");
    exe_outside(dir.path())
        .args(args)
        .output()
        .expect("start stop-slop")
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("stop-slop exits with a code")
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("stdout is UTF-8")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|error| {
        panic!("{error}: {}\nstderr: {}", stdout(out), stderr(out));
    })
}

fn write_config(t: &TempRepo, text: &str) {
    t.write(".git/info/stop-slop/config.toml", text);
}

fn meta(id: &str) -> &'static RuleMeta {
    let def = RULESET.rules.iter().find(|def| def.meta.id == id);
    &def.unwrap_or_else(|| panic!("no rule {id}")).meta
}

fn ids() -> Vec<&'static str> {
    RULESET.rules.iter().map(|def| def.meta.id).collect()
}

/// A rule of the group that works only under a `package.json` with `@nestjs/core`.
fn is_nestjs(id: &str) -> bool {
    meta(id).group == "nestjs"
}

/// A rule of a group that stays off until `enabled` in `config.toml` names it.
fn is_opt_in(id: &str) -> bool {
    RULESET
        .groups
        .iter()
        .any(|group| group.opt_in && group.name == meta(id).group)
}

const NEST_PACKAGE: &str = r#"{"dependencies": {"@nestjs/core": "11.0.0"}}"#;

fn list(rules: &Value) -> &[Value] {
    rules.as_array().expect("an array of rules")
}

fn rule<'v>(rules: &'v Value, id: &str) -> &'v Value {
    let rule = list(rules).iter().find(|rule| rule["id"] == id);
    rule.unwrap_or_else(|| panic!("no rule {id} in {rules}"))
}

fn active(rules: &Value) -> Vec<(&str, bool)> {
    list(rules)
        .iter()
        .map(|rule| {
            let id = rule["id"].as_str().expect("id");
            (id, rule["active"].as_bool().expect("active"))
        })
        .collect()
}

#[test]
fn rules_prints_a_line_per_rule() {
    let t = TempRepo::new();
    let out = run(t.path(), &["rules"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    let words = |id: &str| -> Vec<&str> {
        let line = lines
            .iter()
            .find(|line| line.starts_with(&format!("{id} ")));
        let line = line.unwrap_or_else(|| panic!("no line of {id} in\n{text}"));
        assert!(line.ends_with(meta(id).description), "{line}");
        line.split_whitespace().collect()
    };

    let first_words: Vec<&str> = lines
        .iter()
        .map(|line| line.split(' ').next().unwrap())
        .collect();
    assert_eq!(first_words, ids());
    assert_eq!(words("node-protocol")[1..3], ["error", "fix"]);
    assert_eq!(words("no-any")[1..3], ["error", "-"]);
    assert_eq!(words("one-responsibility")[1], "agent");

    let outside = run_outside(&["rules"]);
    assert_eq!(code(&outside), 0, "{}", stderr(&outside));
    assert_eq!(stdout(&outside), text);
}

#[test]
fn rules_keeps_the_exit_code_when_the_reader_closed_the_pipe() {
    let t = TempRepo::new();
    let (reader, writer) = std::io::pipe().expect("create a pipe");
    drop(reader);
    let out = exe(t.path())
        .arg("rules")
        .stdout(writer)
        .output()
        .expect("start stop-slop");
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}

#[cfg(target_os = "linux")]
#[test]
fn rules_fails_when_stdout_cannot_be_written() {
    let t = TempRepo::new();
    let full = fs::File::options()
        .write(true)
        .open("/dev/full")
        .expect("open /dev/full");
    let out = exe(t.path())
        .arg("rules")
        .stdout(full)
        .output()
        .expect("start stop-slop");
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("stdout: "), "{}", stderr(&out));
}

#[test]
fn rules_json_lists_every_rule() {
    let t = TempRepo::new();
    let out = run(t.path(), &["rules", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let rules = json(&out);

    let keys = BTreeSet::from([
        "id",
        "group",
        "description",
        "kind",
        "fix",
        "files",
        "source",
        "message",
        "agent",
        "active",
    ]);
    for rule in list(&rules) {
        let object = rule.as_object().expect("a rule is an object");
        let rule_keys: BTreeSet<&str> = object.keys().map(String::as_str).collect();
        assert_eq!(rule_keys, keys, "{rule}");
    }
    // No `package.json` depends on `@nestjs/core`, and no `enabled` names an opt-in group.
    let expected: Vec<_> = ids()
        .into_iter()
        .map(|id| (id, !is_nestjs(id) && !is_opt_in(id)))
        .collect();
    assert_eq!(active(&rules), expected);

    let no_any = rule(&rules, "no-any");
    assert_eq!(no_any["group"], "types");
    assert_eq!(no_any["kind"], "error");
    assert_eq!(no_any["fix"], false);
    assert_eq!(no_any["files"], "all");
    assert_eq!(no_any["source"], serde_json::json!(["personal"]));
    assert_eq!(no_any["agent"], Value::Null);
    assert_eq!(no_any["message"], meta("no-any").message);
    assert_eq!(rule(&rules, "node-protocol")["fix"], true);
    let agent = rule(&rules, "one-responsibility");
    assert_eq!(agent["kind"], "agent");
    assert_eq!(agent["agent"], meta("one-responsibility").agent.unwrap());
}

#[test]
fn rules_json_shows_a_disabled_rule_as_inactive() {
    let t = TempRepo::new();
    write_config(&t, "disabled = [\"no-any\"]\n");
    let out = run(t.path(), &["rules", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let expected: Vec<_> = ids()
        .into_iter()
        .map(|id| (id, id != "no-any" && !is_nestjs(id) && !is_opt_in(id)))
        .collect();
    assert_eq!(active(&json(&out)), expected);
}

#[test]
fn rules_json_turns_the_nestjs_group_on_when_one_package_json_has_nestjs_core() {
    let nestjs_states = |t: &TempRepo| -> Vec<bool> {
        let out = run(t.path(), &["rules", "--json"]);
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        active(&json(&out))
            .into_iter()
            .filter(|(id, _)| is_nestjs(id))
            .map(|(_, active)| active)
            .collect()
    };
    let t = TempRepo::new();
    t.write("package.json", "{}")
        .write("apps/api/package.json", NEST_PACKAGE)
        .write("apps/web/package.json", "{}");
    let on = nestjs_states(&t);
    assert!(!on.is_empty());
    assert!(on.iter().all(|&active| active), "{on:?}");

    t.write("apps/api/package.json", "{}");
    let off = nestjs_states(&t);
    assert!(off.iter().all(|&active| !active), "{off:?}");
}

#[test]
fn rules_json_turns_opt_in_groups_on_when_enabled_names_them() {
    let opt_in_states = |t: &TempRepo| -> Vec<bool> {
        let out = run(t.path(), &["rules", "--json"]);
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        active(&json(&out))
            .into_iter()
            .filter(|(id, _)| is_opt_in(id))
            .map(|(_, active)| active)
            .collect()
    };
    let t = TempRepo::new();
    let off = opt_in_states(&t);
    assert!(!off.is_empty());
    assert!(off.iter().all(|&active| !active), "{off:?}");

    let names: Vec<String> = RULESET
        .groups
        .iter()
        .filter(|group| group.opt_in)
        .map(|group| format!("{:?}", group.name))
        .collect();
    write_config(&t, &format!("enabled = [{}]\n", names.join(", ")));
    let on = opt_in_states(&t);
    assert!(on.iter().all(|&active| active), "{on:?}");
}

#[test]
fn rules_json_fails_with_the_cause() {
    let t = TempRepo::new();
    write_config(&t, "disabled = [\"no-such\"]\n");
    let out = run(t.path(), &["rules", "--json"]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("config.toml"), "{}", stderr(&out));
    assert!(stderr(&out).contains("disabled[0]"), "{}", stderr(&out));

    write_config(&t, "disabled = [");
    let out = run(t.path(), &["rules", "--json"]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("config.toml:1:"), "{}", stderr(&out));

    let out = run_outside(&["rules", "--json"]);
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).contains("not inside a git repository"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn rules_json_pretty_indents_the_same_list() {
    let t = TempRepo::new();
    let line = run(t.path(), &["rules", "--json"]);
    let pretty = run(t.path(), &["rules", "--json", "--pretty"]);
    assert_eq!(code(&pretty), 0, "{}", stderr(&pretty));
    let text = stdout(&pretty);
    assert!(text.starts_with("[\n  {\n    \"id\": "), "{text}");
    assert!(text.ends_with("\n]\n"), "{text}");
    assert_eq!(json(&pretty), json(&line));

    let out = run(t.path(), &["rules", "--pretty"]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("--json"), "{}", stderr(&out));
}

fn check(dir: &Path, args: &[&str], expected: i32) -> Value {
    let out = run(dir, args);
    assert_eq!(code(&out), expected, "{args:?}: {}", stderr(&out));
    json(&out)
}

fn sha7(t: &TempRepo, rev: &str) -> String {
    t.git(&["rev-parse", rev])[..7].to_owned()
}

fn is_fingerprint(value: &Value, rule: &str) -> bool {
    value
        .as_str()
        .and_then(|text| text.strip_prefix(rule)?.strip_prefix(':'))
        .is_some_and(|hash| hash.len() == 6 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn findings(report: &Value) -> &[Value] {
    report["findings"].as_array().expect("an array of findings")
}

fn finding_files(report: &Value) -> Vec<&str> {
    findings(report)
        .iter()
        .map(|finding| finding["file"].as_str().expect("file"))
        .collect()
}

/// `main` with `src/keep.constants.ts`; `feat` changes line 1 of it, keeps the `any` on line 2 and
/// adds `src/new.constants.ts` with a `TODO` without an owner.
fn branch_repo() -> TempRepo {
    branch_repo_with(&[])
}

/// `branch_repo` with `main_files` added to `main` in a commit of their own after
/// `keep.constants.ts`; `feat` does not touch them.
fn branch_repo_with(main_files: &[(&str, &str)]) -> TempRepo {
    let t = TempRepo::new();
    t.write(
        "src/keep.constants.ts",
        "export const ALPHA = 1;\nexport const BETA: any = 2;\n",
    )
    .commit("main");
    if !main_files.is_empty() {
        for (path, text) in main_files {
            t.write(path, text);
        }
        t.commit("main files");
    }
    t.branch("feat")
        .write(
            "src/keep.constants.ts",
            "export const ALPHA = 10;\nexport const BETA: any = 2;\n",
        )
        .write(
            "src/new.constants.ts",
            "// TODO fix\nexport const FRESH = 1;\n",
        )
        .commit("feat");
    t
}

#[test]
fn check_reports_findings_of_the_branch_as_json() {
    let t = branch_repo();
    let report = check(t.path(), &["check", "--base", "main", "--json"], 1);

    let keys: BTreeSet<&str> = report
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from(["base", "files", "skipped", "findings", "fixed"])
    );
    assert_eq!(
        report["base"],
        json!({"ref": "main", "sha": sha7(&t, "main"), "reason": "flag"})
    );
    assert_eq!(
        report["files"],
        json!(["src/keep.constants.ts", "src/new.constants.ts"])
    );
    assert_eq!(report["skipped"], json!([]));
    assert_eq!(report["fixed"], json!([]));

    let [any, todo] = findings(&report) else {
        panic!("two findings: {report}");
    };
    assert!(is_fingerprint(&any["fingerprint"], "no-any"), "{any}");
    assert!(is_fingerprint(&todo["fingerprint"], "todo-owner"), "{todo}");
    assert_eq!(
        *any,
        json!({
            "rule": "no-any",
            "kind": "error",
            "file": "src/keep.constants.ts",
            "line": 2,
            "column": 20,
            "endLine": 2,
            "endColumn": 23,
            "message": meta("no-any").message,
            "inHunk": false,
            "fixable": false,
            "fingerprint": any["fingerprint"],
        })
    );
    assert_eq!(
        *todo,
        json!({
            "rule": "todo-owner",
            "kind": "error",
            "file": "src/new.constants.ts",
            "line": 1,
            "column": 4,
            "endLine": 1,
            "endColumn": 12,
            "message": meta("todo-owner").message,
            "inHunk": true,
            "fixable": false,
            "fingerprint": todo["fingerprint"],
        })
    );
}

#[test]
fn check_json_is_one_line_and_pretty_indents_it() {
    let t = branch_repo();
    let line = run(t.path(), &["check", "--base", "main", "--json"]);
    assert_eq!(code(&line), 1, "{}", stderr(&line));
    let text = stdout(&line);
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(text.ends_with("}\n"), "{text}");

    let pretty = run(t.path(), &["check", "--base", "main", "--json", "--pretty"]);
    assert_eq!(code(&pretty), 1, "{}", stderr(&pretty));
    let text = stdout(&pretty);
    assert!(
        text.starts_with("{\n  \"base\": {\n    \"ref\": \"main\","),
        "{text}"
    );
    assert!(text.ends_with("\n}\n"), "{text}");
    assert_eq!(json(&pretty), json(&line));
}

#[test]
fn check_prints_the_base_and_the_findings_as_text() {
    let t = TempRepo::new();
    t.write("src/m.constants.ts", "export const MAIN = 1;\n")
        .commit("main");
    t.branch("feat")
        .write("src/clean.constants.ts", "export const CLEAN = 1;\n")
        .commit("feat");
    // No `origin/feat`, so the base is searched without `gh`.
    let out = run(t.path(), &["check"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!("base main {} (ancestor)\n", sha7(&t, "main"))
    );

    let t = branch_repo();
    let out = run(t.path(), &["check", "--base", "main"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], format!("base main {} (flag)", sha7(&t, "main")));
    for line in [
        format!(
            "src/keep.constants.ts:2:20 no-any {}",
            meta("no-any").message
        ),
        format!(
            "src/new.constants.ts:1:4 todo-owner {}",
            meta("todo-owner").message
        ),
    ] {
        assert!(lines.contains(&line.as_str()), "no `{line}` in\n{text}");
    }
}

#[test]
fn check_rejects_flags_that_do_not_go_together() {
    let t = branch_repo();
    for args in [
        &["check", "--rev", "feat"][..],
        &["check", "--files", "src/new.constants.ts", "--all"],
        &["check", "--base", "main", "--files", "src/new.constants.ts"],
        &["check", "--base", "main", "--all"],
        &["check", "--pretty"],
    ] {
        let out = run(t.path(), args);
        assert_eq!(code(&out), 2, "{args:?}");
        assert!(!stderr(&out).is_empty(), "{args:?}");
    }
}

#[test]
fn check_without_a_base_checks_every_line() {
    let t = branch_repo();
    let report = check(t.path(), &["check", "--all", "--json"], 1);
    assert_eq!(report["base"], Value::Null);
    assert_eq!(report["skipped"], json!([]));
    assert_eq!(
        report["files"],
        json!(["src/keep.constants.ts", "src/new.constants.ts"])
    );
    assert!(!findings(&report).is_empty());
    for finding in findings(&report) {
        assert_eq!(finding["inHunk"], Value::Null, "{finding}");
    }

    let absolute = t.path().join("src/new.constants.ts");
    let src = t.path().join("src");
    for (dir, file) in [
        (t.path(), "src/new.constants.ts"),
        (src.as_path(), "new.constants.ts"),
        (t.path(), absolute.to_str().expect("a UTF-8 temp path")),
    ] {
        let report = check(dir, &["check", "--files", file, "--json"], 1);
        assert_eq!(report["files"], json!(["src/new.constants.ts"]), "{file}");
        assert_eq!(report["base"], Value::Null, "{file}");
        assert_eq!(finding_files(&report), ["src/new.constants.ts"], "{file}");
        assert_eq!(findings(&report)[0]["inHunk"], Value::Null, "{file}");
    }

    t.write("README.md", "# readme\n");
    let args = [
        "check",
        "--files",
        "src/new.constants.ts",
        "README.md",
        "--json",
    ];
    assert_eq!(
        check(t.path(), &args, 1)["files"],
        json!(["src/new.constants.ts"])
    );

    t.write("src/gone.constants.ts", "export const GONE: any = 1;\n")
        .commit("gone");
    fs::remove_file(t.path().join("src/gone.constants.ts")).expect("remove src/gone.constants.ts");
    let report = check(t.path(), &["check", "--all", "--json"], 1);
    assert_eq!(
        report["files"],
        json!(["src/keep.constants.ts", "src/new.constants.ts"])
    );
}

#[test]
fn check_fails_on_a_broken_config_or_ignore_file() {
    let t = branch_repo();
    for (config, key) in [
        ("disabled = [\"no-such\"]\n", "disabled[0]"),
        ("[rules.no-any]\nx = 1\n", "rules.no-any.x"),
        ("enabled = [\"no-such\"]\n", "enabled[0]"),
    ] {
        write_config(&t, config);
        let out = run(t.path(), &["check", "--base", "main"]);
        assert_eq!(code(&out), 2, "{config}");
        assert!(stderr(&out).contains(key), "{}", stderr(&out));
    }

    write_config(&t, "");
    t.write(".git/info/stop-slop/ignore.json", "{}");
    let out = run(t.path(), &["check", "--base", "main"]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("ignore.json:1:"), "{}", stderr(&out));
}

#[test]
fn check_drops_ignored_findings_and_disabled_rules() {
    let t = TempRepo::new();
    t.write("src/m.constants.ts", "export const MAIN = 1;\n")
        .commit("main");
    t.branch("feat")
        .write(
            "src/one.constants.ts",
            "// TODO fix\nexport const ONE = 1;\n",
        )
        .write(
            "src/two.constants.ts",
            "// TODO fix\nexport const TWO = 2;\n",
        )
        .commit("feat");
    let args = ["check", "--base", "main", "--json"];

    let report = check(t.path(), &args, 1);
    let [one, two] = findings(&report) else {
        panic!("two findings: {report}");
    };
    assert_eq!(one["rule"], "todo-owner");
    assert_eq!(two["rule"], "todo-owner");
    assert_eq!(one["fingerprint"], two["fingerprint"]);

    let fingerprint = one["fingerprint"].as_str().expect("fingerprint").to_owned();
    State::open(&t.path().join(".git"))
        .add_ignore(vec![IgnoreEntry {
            fingerprint,
            rule: "todo-owner".into(),
            file: "src/one.constants.ts".into(),
            reason: "test".into(),
            added: "2026-10-15".into(),
        }])
        .expect("write ignore.json");
    assert_eq!(
        finding_files(&check(t.path(), &args, 1)),
        ["src/two.constants.ts"]
    );

    write_config(&t, "disabled = [\"todo-owner\"]\n");
    assert!(findings(&check(t.path(), &args, 0)).is_empty());
}

#[test]
fn check_at_a_revision_reads_the_files_from_git() {
    let t = TempRepo::new();
    t.write("src/m.constants.ts", "export const MAIN = 1;\n")
        .commit("main");
    t.branch("feat")
        .write("src/r.constants.ts", "export const REVISION: any = 1;\n")
        .commit("feat");
    t.write("src/r.constants.ts", "export const REVISION = 1;\n")
        .write("src/u.constants.ts", "export const UNTRACKED: any = 1;\n");

    let args = ["check", "--rev", "feat", "--base", "main", "--json"];
    let report = check(t.path(), &args, 1);
    assert_eq!(report["files"], json!(["src/r.constants.ts"]));
    assert_eq!(report["base"]["reason"], "flag");
    let [finding] = findings(&report) else {
        panic!("one finding: {report}");
    };
    assert_eq!(finding["rule"], "no-any");
    assert_eq!(finding["file"], "src/r.constants.ts");
    assert_eq!(finding["inHunk"], true);
    assert_eq!(
        read(&t, "src/r.constants.ts"),
        "export const REVISION = 1;\n"
    );

    let report = check(t.path(), &["check", "--base", "main", "--json"], 1);
    assert_eq!(finding_files(&report), ["src/u.constants.ts"]);
}

#[test]
fn check_fails_with_the_cause() {
    let out = run_outside(&["check"]);
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).contains("not inside a git repository"),
        "{}",
        stderr(&out)
    );

    let t = branch_repo();
    for (args, cause) in [
        (
            &["check", "--base", "no-such"][..],
            "base `no-such` is not a commit",
        ),
        (&["check", "--rev", "no-such", "--base", "main"], "no-such"),
        (
            &["check", "--files", "src/missing.constants.ts"],
            "missing.constants.ts",
        ),
        (&["check", "--files", "src"], "src: is a directory"),
    ] {
        let out = run(t.path(), args);
        assert_eq!(code(&out), 2, "{args:?}");
        assert!(stderr(&out).contains(cause), "{args:?}: {}", stderr(&out));
    }
}

#[test]
fn check_reports_a_file_that_does_not_parse() {
    let t = TempRepo::new();
    t.write("src/m.constants.ts", "export const MAIN = 1;\n")
        .commit("main");
    t.branch("feat")
        .write("src/bad.constants.ts", "const = ;\n")
        .commit("feat");
    let report = check(t.path(), &["check", "--base", "main", "--json"], 1);
    let [finding] = findings(&report) else {
        panic!("one finding: {report}");
    };
    assert_eq!(finding["rule"], "parse-error");
    assert_eq!(finding["kind"], "parse-error");
    assert_eq!(finding["file"], "src/bad.constants.ts");
}

#[test]
fn check_skips_files_a_squash_merge_brought_into_the_base() {
    let t = TempRepo::new();
    t.write("shared.constants.ts", "1\n2\n3\n")
        .write("m.constants.ts", "m\n")
        .commit("main");
    t.branch("a")
        .write("a.constants.ts", "a\n")
        .write("shared.constants.ts", "one\n2\n3\n")
        .commit("a");
    t.branch("b")
        .write("b.constants.ts", "b\n")
        .write("shared.constants.ts", "one\n2\nthree\n")
        .commit("b");
    t.checkout("main");
    t.git(&["merge", "-q", "--squash", "a"]);
    t.commit("squash a");
    t.git(&["branch", "-q", "-D", "a"]);
    t.checkout("b");

    let report = check(t.path(), &["check", "--base", "main", "--json"], 0);
    assert_eq!(
        report["files"],
        json!(["b.constants.ts", "shared.constants.ts"])
    );
    assert_eq!(
        report["skipped"],
        json!([{"file": "a.constants.ts", "reason": "already-in-base"}])
    );
}

#[test]
fn check_finds_a_raw_value_of_an_enum_from_outside_the_diff() {
    let t = TempRepo::new();
    t.write(
        "src/source.enum.ts",
        "export enum Source {\n  Dashboard = 'DASHBOARD',\n}\n",
    )
    .commit("main");
    t.branch("feat")
        .write(
            "src/sync.constants.ts",
            "import { Source } from './source.enum';\n\n\
             export const SOURCE: Source | string = 'DASHBOARD';\n",
        )
        // The file does not know the enum, so the same word is no finding.
        .write(
            "src/label.constants.ts",
            "export const LABEL = 'DASHBOARD';\n",
        )
        .commit("feat");

    let report = check(t.path(), &["check", "--base", "main", "--json"], 1);
    assert_eq!(
        report["files"],
        json!(["src/label.constants.ts", "src/sync.constants.ts"])
    );
    assert_eq!(
        places(&report),
        [(
            "no-enum-raw-value",
            "src/sync.constants.ts",
            3,
            40,
            Some(true)
        )]
    );
    let finding = &findings(&report)[0];
    assert_eq!(finding["kind"], "candidate");
    let message = finding["message"].as_str().expect("a message");
    assert!(message.contains("`Source.Dashboard`"), "{message}");
}

#[test]
fn check_runs_the_nestjs_rules_only_under_a_package_with_nestjs_core() {
    let controller = "import { Controller, Get } from '@nestjs/common';\n\n\
                      @Controller('reports')\nexport class ReportsController {\n  \
                      @Get()\n  list(): string[] {\n    return [];\n  }\n}\n";
    let t = TempRepo::new();
    t.write("apps/api/package.json", NEST_PACKAGE)
        .write("apps/api/src/reports.controller.ts", controller)
        .write("apps/web/package.json", "{}")
        .write("apps/web/src/reports.controller.ts", controller)
        .commit("main");

    let report = check(t.path(), &["check", "--all", "--json"], 1);
    let files: Vec<&str> = findings(&report)
        .iter()
        .filter(|finding| finding["rule"] == "nest-http-code")
        .map(|finding| finding["file"].as_str().expect("file"))
        .collect();
    assert_eq!(files, ["apps/api/src/reports.controller.ts"]);
}

#[test]
fn check_names_the_facts_of_a_lookup_in_a_loop_once_enabled_turns_performance_on() {
    let t = TempRepo::new();
    t.write(
        "src/orders.ts",
        "export function kept(orders: { id: string }[], ids: string[]): { id: string }[] {\n  \
         return orders.filter((order) => ids.includes(order.id));\n}\n",
    )
    .commit("main");
    let lookups = |t: &TempRepo| messages(t, "src/orders.ts", "no-lookup-in-loop");
    assert!(lookups(&t).is_empty());

    write_config(&t, "enabled = [\"performance\"]\n");
    let found = lookups(&t);
    let [message] = found.as_slice() else {
        panic!("one lookup: {found:?}");
    };
    for fact in [
        "`ids` is a parameter",
        "`ids.includes(..)`",
        "`filter` callback over `orders`",
        "loop depth 1",
        "by value",
    ] {
        assert!(message.contains(fact), "{fact}: {message}");
    }
}

#[test]
fn check_names_what_an_await_in_a_loop_carries_over_and_how_the_loop_ends() {
    let t = TempRepo::new();
    t.write(
        "src/pages.ts",
        "declare function fetchPage(cursor: string): Promise<{ next: string; sizes: number[] }>;\n\
         export async function walk(): Promise<number> {\n  \
           let total = 0;\n  \
           for (let cursor = 'start'; cursor !== ''; ) {\n    \
             const page = await fetchPage(cursor);\n    \
             const sorted = page.sizes.sort((left, right) => {\n      \
               let order = left - right;\n      \
               order *= -1;\n      \
               return order;\n    \
             });\n    \
             total += sorted[0] ?? 0;\n    \
             cursor = page.next;\n    \
             if (total > 100) break;\n  \
           }\n  \
           return total;\n\
         }\n",
    )
    .commit("main");
    write_config(&t, "enabled = [\"performance\"]\n");

    let found = messages(&t, "src/pages.ts", "no-await-in-loop");
    let [message] = found.as_slice() else {
        panic!("one await: {found:?}");
    };
    for fact in [
        "`await fetchPage(..)`",
        "`fetchPage`, which by its name looks like a round trip",
        // `order` is a local of the comparator, not of the loop.
        "the body writes: `total`, `cursor`.",
        "Early exit: `break`.",
    ] {
        assert!(message.contains(fact), "{fact}: {message}");
    }
}

#[test]
fn check_holds_cognitive_complexity_to_the_max_of_config_toml() {
    let t = TempRepo::new();
    t.write(
        "src/sizes.ts",
        "export function size(rows: { skip: boolean }[]): number {\n  \
           let total = 0;\n  \
           for (const row of rows) {\n    \
             if (row.skip) continue;\n    \
             total += 1;\n  \
           }\n  \
           return total;\n\
         }\n",
    )
    .commit("main");
    let complexity = || messages(&t, "src/sizes.ts", "max-cognitive-complexity");
    write_config(&t, "enabled = [\"branching\"]\n");
    assert!(complexity().is_empty());

    write_config(
        &t,
        "enabled = [\"branching\"]\n\n[rules.max-cognitive-complexity]\nmax = 2\n",
    );
    let found = complexity();
    let [message] = found.as_slice() else {
        panic!("one function: {found:?}");
    };
    assert!(
        message.starts_with("`size` has a cognitive complexity of 3, past the limit of 2"),
        "{message}"
    );
}

fn messages(t: &TempRepo, file: &str, rule: &str) -> Vec<String> {
    let out = run(t.path(), &["check", "--files", file, "--json"]);
    assert_ne!(code(&out), 2, "{}", stderr(&out));
    findings(&json(&out))
        .iter()
        .filter(|finding| finding["rule"] == rule)
        .map(|finding| finding["message"].as_str().expect("a message").to_owned())
        .collect()
}

const IO_FEAT: &str = "import { readFileSync } from 'fs'; export const read: any = readFileSync;\n\
                       export const ALPHA = 10;\nexport const BETA: any = 2;\n";
const MAIN_FEAT: &str =
    "import path from 'path';\nimport os from 'os';\nexport const modules = [path, os];\n";

/// `main` with `src/io.constants.ts`: a bare `'fs'` and an `any` on line 1, an `any` on line 3.
/// `feat` changes line 2 of it and adds `src/main.ts` with two bare imports.
fn fix_repo() -> TempRepo {
    let t = TempRepo::new();
    t.write(
        "src/io.constants.ts",
        &IO_FEAT.replace("ALPHA = 10", "ALPHA = 1"),
    )
    .commit("main");
    t.branch("feat")
        .write("src/io.constants.ts", IO_FEAT)
        .write("src/main.ts", MAIN_FEAT)
        .commit("feat");
    t
}

fn read(t: &TempRepo, path: &str) -> String {
    fs::read_to_string(t.path().join(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// Rule, file, line, column and `inHunk` of each finding.
fn places(report: &Value) -> Vec<(&str, &str, u64, u64, Option<bool>)> {
    findings(report)
        .iter()
        .map(|finding| {
            (
                finding["rule"].as_str().expect("rule"),
                finding["file"].as_str().expect("file"),
                finding["line"].as_u64().expect("line"),
                finding["column"].as_u64().expect("column"),
                finding["inHunk"].as_bool(),
            )
        })
        .collect()
}

#[test]
fn check_fix_writes_the_fixes_and_checks_the_new_text() {
    let t = fix_repo();
    let before = check(t.path(), &["check", "--base", "main", "--json"], 1);
    assert_eq!(
        places(&before),
        [
            ("node-protocol", "src/io.constants.ts", 1, 30, Some(false)),
            ("no-any", "src/io.constants.ts", 1, 55, Some(false)),
            ("no-any", "src/io.constants.ts", 3, 20, Some(false)),
            ("node-protocol", "src/main.ts", 1, 18, Some(true)),
            ("node-protocol", "src/main.ts", 2, 16, Some(true)),
        ]
    );
    assert_eq!(findings(&before)[0]["fixable"], true);

    let args = ["check", "--base", "main", "--fix", "--json"];
    let report = check(t.path(), &args, 1);
    let io_fixed = IO_FEAT.replace("'fs'", "'node:fs'");
    let main_fixed = "import path from 'node:path';\nimport os from 'node:os';\n\
                      export const modules = [path, os];\n";
    assert_eq!(read(&t, "src/io.constants.ts"), io_fixed);
    assert_eq!(read(&t, "src/main.ts"), main_fixed);
    assert_eq!(
        report["fixed"],
        json!([
            {"rule": "node-protocol", "file": "src/io.constants.ts", "count": 1},
            {"rule": "node-protocol", "file": "src/main.ts", "count": 2},
        ])
    );
    // `--fix` changed line 1 of `src/io.constants.ts`, so it is in the diff now.
    assert_eq!(
        places(&report),
        [
            ("no-any", "src/io.constants.ts", 1, 60, Some(true)),
            ("no-any", "src/io.constants.ts", 3, 20, Some(false)),
        ]
    );
    assert_eq!(
        report["base"],
        json!({"ref": "main", "sha": sha7(&t, "main"), "reason": "flag"})
    );
    assert_eq!(
        report["files"],
        json!(["src/io.constants.ts", "src/main.ts"])
    );

    let again = check(t.path(), &args, 1);
    assert_eq!(again["fixed"], json!([]));
    assert_eq!(again["findings"], report["findings"]);
    assert_eq!(read(&t, "src/io.constants.ts"), io_fixed);
    assert_eq!(read(&t, "src/main.ts"), main_fixed);

    t.git(&["checkout", "--", "src"]);
    let out = run(t.path(), &["check", "--base", "main", "--fix"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    for line in [
        "fixed src/io.constants.ts node-protocol x1",
        "fixed src/main.ts node-protocol x2",
    ] {
        assert!(lines.contains(&line), "no `{line}` in\n{text}");
    }
}

#[test]
fn check_fix_of_files_fixes_only_those_files() {
    let t = fix_repo();
    let args = ["check", "--files", "src/main.ts", "--fix", "--json"];
    let report = check(t.path(), &args, 0);
    assert_eq!(
        report["fixed"],
        json!([{"rule": "node-protocol", "file": "src/main.ts", "count": 2}])
    );
    assert_eq!(report["base"], Value::Null);
    assert_eq!(report["findings"], json!([]));
    assert_eq!(read(&t, "src/io.constants.ts"), IO_FEAT);
}

#[test]
fn check_fix_at_a_revision_is_refused() {
    let t = fix_repo();
    let out = run(
        t.path(),
        &["check", "--rev", "feat", "--base", "main", "--fix"],
    );
    assert_eq!(code(&out), 2);
    assert!(!stderr(&out).is_empty());
    assert_eq!(read(&t, "src/io.constants.ts"), IO_FEAT);
    assert_eq!(read(&t, "src/main.ts"), MAIN_FEAT);
}

#[test]
fn check_fix_keeps_crlf_line_endings() {
    let t = TempRepo::new();
    t.write("src/m.constants.ts", "export const MAIN = 1;\n")
        .commit("main");
    t.branch("feat")
        .write(
            "src/crlf.constants.ts",
            "import os from 'os';\r\nexport const system = os;\r\n",
        )
        .commit("feat");

    let report = check(t.path(), &["check", "--base", "main", "--fix", "--json"], 0);
    assert_eq!(
        report["fixed"],
        json!([{"rule": "node-protocol", "file": "src/crlf.constants.ts", "count": 1}])
    );
    assert_eq!(
        read(&t, "src/crlf.constants.ts"),
        "import os from 'node:os';\r\nexport const system = os;\r\n"
    );
}

#[test]
fn check_fix_drops_a_file_that_became_as_in_the_base() {
    let t = TempRepo::new();
    let base_text = "import fs from 'node:fs';\nexport const files = fs;\n";
    t.write("src/x.constants.ts", base_text).commit("main");
    t.branch("feat")
        .write("src/x.constants.ts", &base_text.replace("node:", ""))
        .write("src/y.constants.ts", "export const EXTRA = 1;\n")
        .commit("feat");

    let report = check(t.path(), &["check", "--base", "main", "--fix", "--json"], 0);
    assert_eq!(read(&t, "src/x.constants.ts"), base_text);
    assert_eq!(
        report["fixed"],
        json!([{"rule": "node-protocol", "file": "src/x.constants.ts", "count": 1}])
    );
    assert_eq!(report["files"], json!(["src/y.constants.ts"]));
    assert_eq!(report["skipped"], json!([]));
}

const ANY: &str = "export const ALPHA = 1;\nexport const BETA: any = 2;\n";

fn hook_input(file: &Path) -> String {
    let file = file.to_str().expect("a UTF-8 path");
    json!({"hook_event_name": "PostToolUse", "tool_name": "Edit", "tool_input": {"file_path": file}})
        .to_string()
}

fn hook(dir: &Path, input: &str) -> Output {
    feed(exe(dir), input)
}

fn feed(mut command: Command, input: &str) -> Output {
    let mut child = command
        .arg("hook")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start stop-slop");
    let mut stdin = child.stdin.take().expect("a piped stdin");
    stdin
        .write_all(input.as_bytes())
        .expect("write the hook input");
    drop(stdin);
    child.wait_with_output().expect("wait for stop-slop")
}

fn enable(t: &TempRepo) {
    let out = run(t.path(), &["enable"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}

fn marker(t: &TempRepo) -> PathBuf {
    t.path().join(".git/info/stop-slop/enabled")
}

/// Code and stderr of the hook on `file` of `t`, from its root.
fn hook_on(t: &TempRepo, file: &str) -> (i32, String) {
    let out = hook(t.path(), &hook_input(&t.path().join(file)));
    assert!(out.stdout.is_empty(), "{}", stdout(&out));
    (code(&out), stderr(&out))
}

#[cfg(unix)]
fn symlink_file_or_skip(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).unwrap();
    true
}

/// A symlink on Windows needs admin rights or Developer Mode; without them the case is
/// skipped. The GitHub Actions runner is admin, so CI runs it.
#[cfg(windows)]
fn symlink_file_or_skip(target: &Path, link: &Path) -> bool {
    const ERROR_PRIVILEGE_NOT_HELD: i32 = 1314;
    match std::os::windows::fs::symlink_file(target, link) {
        Ok(()) => true,
        Err(error) if error.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD) => {
            eprintln!("skipped: no right to create a symlink ({error})");
            false
        }
        Err(error) => panic!("symlink {}: {error}", link.display()),
    }
}

#[test]
fn enable_and_disable_change_only_the_marker() {
    let t = TempRepo::new();
    write_config(&t, "disabled = []\n");
    t.write(".git/info/stop-slop/ignore.json", "[]\n")
        .write("src/a.constants.ts", ANY);
    let state_files = || {
        ["config.toml", "ignore.json"].map(|name| {
            let path = t.path().join(".git/info/stop-slop").join(name);
            fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        })
    };
    let before = state_files();
    let src = t.path().join("src");

    for (command, enabled) in [
        ("enable", true),
        ("enable", true),
        ("disable", false),
        ("disable", false),
    ] {
        let out = run(&src, &[command]);
        assert_eq!(code(&out), 0, "{command}: {}", stderr(&out));
        assert_eq!(stdout(&out), "", "{command}");
        assert_eq!(marker(&t).is_file(), enabled, "{command}");
        assert_eq!(state_files(), before, "{command}");
    }
}

#[test]
fn enable_and_disable_outside_a_repo_fail() {
    for command in ["enable", "disable"] {
        let out = run_outside(&[command]);
        assert_eq!(code(&out), 2, "{command}");
        assert!(
            stderr(&out).contains("not inside a git repository"),
            "{command}: {}",
            stderr(&out)
        );
    }
}

/// A worktree of `t` on a new branch `side`; it lives as long as the returned temp folder.
fn linked_worktree(t: &TempRepo) -> (tempfile::TempDir, PathBuf) {
    let trees = tempfile::tempdir().expect("create a temp folder");
    let tree = trees.path().join("tree");
    let tree_arg = tree.to_str().expect("a UTF-8 temp path");
    t.git(&["worktree", "add", "-q", tree_arg, "-b", "side"]);
    (trees, tree)
}

#[test]
fn enable_in_a_linked_worktree_turns_the_hook_on_for_the_repo() {
    let t = TempRepo::new();
    t.write("src/a.constants.ts", ANY).commit("main");
    let (_trees, tree) = linked_worktree(&t);

    let out = run(&tree, &["enable"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(marker(&t).is_file());
    assert!(tree.join(".git").is_file());

    assert_eq!(hook_on(&t, "src/a.constants.ts").0, 2);
    let out = hook(&tree, &hook_input(&tree.join("src").join("a.constants.ts")));
    assert_eq!(code(&out), 2, "{}", stderr(&out));
}

#[test]
fn config_error_in_a_linked_worktree_names_the_file_of_the_repo() {
    let t = TempRepo::new();
    t.write("src/a.constants.ts", ANY).commit("main");
    let (_trees, tree) = linked_worktree(&t);
    write_config(&t, "disabled = [\"no-such\"]\n");

    let out = run(&tree, &["check", "--all"]);
    assert_eq!(code(&out), 2);
    let text = stderr(&out);
    let path = text
        .strip_prefix("stop-slop: ")
        .and_then(|rest| rest.split_once(": unknown rule `disabled[0]`"))
        .map_or_else(|| panic!("{text}"), |(path, _)| path);
    let canonical = |path: &Path| {
        fs::canonicalize(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    };
    assert_eq!(
        canonical(Path::new(path)),
        canonical(&t.path().join(".git/info/stop-slop/config.toml"))
    );
}

#[test]
fn hook_reports_errors_with_code_2() {
    let t = TempRepo::new();
    enable(&t);
    t.write("src/a.constants.ts", ANY);
    let expected = format!(
        "src/a.constants.ts:2:20 no-any {}\n",
        meta("no-any").message
    );

    let absolute = t.path().join("src").join("a.constants.ts");
    for input in [
        hook_input(&absolute),
        hook_input(Path::new("src/a.constants.ts")),
    ] {
        let out = hook(t.path(), &input);
        assert_eq!(code(&out), 2, "{input}: {}", stderr(&out));
        assert_eq!(stdout(&out), "", "{input}");
        assert_eq!(stderr(&out), expected, "{input}");
    }
}

#[test]
fn hook_prints_at_most_20_lines() {
    let t = TempRepo::new();
    enable(&t);
    let text: String = (1..=25)
        .map(|i| format!("export const A{i}: any = {i};\n"))
        .collect();
    t.write("src/many.constants.ts", &text);

    let (code, text) = hook_on(&t, "src/many.constants.ts");
    assert_eq!(code, 2, "{text}");
    let message = meta("no-any").message;
    let mut expected: Vec<String> = (1..=19)
        .map(|i| {
            let column = 17 + i.to_string().len();
            format!("src/many.constants.ts:{i}:{column} no-any {message}")
        })
        .collect();
    expected.push("stop-slop: 6 more errors".into());
    assert_eq!(text.lines().collect::<Vec<_>>(), expected);
}

#[test]
fn hook_passes_what_it_does_not_check() {
    let t = TempRepo::new();
    t.write("src/a.constants.ts", ANY);
    assert_eq!(
        hook_on(&t, "src/a.constants.ts"),
        (0, String::new()),
        "no marker"
    );

    enable(&t);
    let draft = ".git/info/stop-slop/reviews/pr-1.md";
    t.write("notes.md", "# any\n")
        .write("src/types.d.ts", "export type T = any;\n")
        .write(draft, "# review\n")
        .write("src/clean.constants.ts", "export const CLEAN = 1;\n");
    for file in [
        "notes.md",
        "src/types.d.ts",
        draft,
        "src/clean.constants.ts",
    ] {
        assert_eq!(hook_on(&t, file), (0, String::new()), "{file}");
    }

    let outside = tempfile::tempdir().expect("create a temp folder");
    let file = outside.path().join("a.constants.ts");
    fs::write(&file, ANY).expect("write a.constants.ts");
    if symlink_file_or_skip(&file, &t.path().join("src/link.constants.ts")) {
        assert_eq!(
            hook_on(&t, "src/link.constants.ts"),
            (0, String::new()),
            "symlink"
        );
    }
    let out = feed(exe_outside(outside.path()), &hook_input(&file));
    assert_eq!(code(&out), 0, "outside a repo: {}", stderr(&out));
    assert_eq!(stderr(&out), "", "outside a repo");

    let out = run(t.path(), &["disable"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        hook_on(&t, "src/a.constants.ts"),
        (0, String::new()),
        "disabled"
    );
}

#[test]
fn hook_passes_a_folder_git_refuses_but_fails_without_git() {
    let t = TempRepo::new();
    enable(&t);
    let in_git = ".git/info/stop-slop/reviews/x.constants.ts";
    t.write("src/a.constants.ts", ANY).write(in_git, ANY);
    assert_eq!(hook_on(&t, in_git), (0, String::new()), "inside .git");

    let empty = tempfile::tempdir().expect("create a temp folder");
    let no_config = empty.path().join("config");
    fs::write(&no_config, "").expect("write an empty git config");
    // `safe.directory` in the system or the user's git config could let the repo in.
    let foreign = || {
        let mut command = exe(t.path());
        command
            .env("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &no_config);
        command
    };
    let out = foreign()
        .args(["check", "--all"])
        .output()
        .expect("start stop-slop");
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).contains("dubious ownership"),
        "{}",
        stderr(&out)
    );
    let out = feed(foreign(), &hook_input(&t.path().join("src/a.constants.ts")));
    assert_eq!((code(&out), stderr(&out)), (0, String::new()), "foreign");

    let mut no_git = exe(t.path());
    no_git.env("PATH", empty.path());
    let out = feed(no_git, &hook_input(&t.path().join("src/a.constants.ts")));
    assert_eq!(code(&out), 2, "no git");
    assert!(stderr(&out).contains("git"), "{}", stderr(&out));
}

#[test]
fn hook_uses_the_repo_of_the_file() {
    let a = TempRepo::new();
    enable(&a);
    a.write("src/a.constants.ts", ANY);
    let b = TempRepo::new();
    b.write("src/b.constants.ts", ANY);

    let out = hook(b.path(), &hook_input(&a.path().join("src/a.constants.ts")));
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    let out = hook(a.path(), &hook_input(&b.path().join("src/b.constants.ts")));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}

#[test]
fn hook_fails_with_the_cause() {
    let t = TempRepo::new();
    t.write("src/a.constants.ts", ANY);
    for input in ["not json", r#"{"tool_input": {}}"#] {
        let out = hook(t.path(), input);
        assert_eq!(code(&out), 2, "{input}");
        assert!(
            stderr(&out).contains("hook input"),
            "{input}: {}",
            stderr(&out)
        );
    }

    write_config(&t, "disabled = [\"no-such\"]\n");
    assert_eq!(
        hook_on(&t, "src/a.constants.ts"),
        (0, String::new()),
        "no marker"
    );
    enable(&t);
    let (code, text) = hook_on(&t, "src/a.constants.ts");
    assert_eq!(code, 2);
    assert!(text.contains("config.toml"), "{text}");
    assert!(text.contains("disabled[0]"), "{text}");
}

#[test]
fn hook_reports_a_file_that_does_not_parse() {
    let t = TempRepo::new();
    enable(&t);
    t.write("src/bad.constants.ts", "const = ;\n");
    let (code, text) = hook_on(&t, "src/bad.constants.ts");
    assert_eq!(code, 2, "{text}");
    let [line] = text.lines().collect::<Vec<_>>()[..] else {
        panic!("one line: {text}");
    };
    assert!(line.starts_with("src/bad.constants.ts:1:"), "{line}");
    assert!(line.contains(" parse-error "), "{line}");
}

/// The command of each `git` run, from a `GIT_TRACE` file; none when there is no file.
fn git_commands(trace: &Path) -> Vec<String> {
    let text = fs::read_to_string(trace).unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let (_, command) = line.split_once("trace: built-in: git ")?;
            command.split(' ').next()
        })
        .map(str::to_owned)
        .collect()
}

#[test]
fn hook_runs_git_once_and_not_for_other_files() {
    let t = TempRepo::new();
    enable(&t);
    let draft = ".git/info/stop-slop/reviews/pr-1.md";
    t.write("src/a.constants.ts", ANY)
        .write("notes.md", "# notes\n")
        .write(draft, "# review\n");
    let traces = tempfile::tempdir().expect("create a temp folder");
    let traced = |file: &str, name: &str| {
        let trace = traces.path().join(name);
        let mut command = exe(t.path());
        command.env("GIT_TRACE", &trace);
        let out = feed(command, &hook_input(&t.path().join(file)));
        (code(&out), git_commands(&trace))
    };

    assert_eq!(
        traced("src/a.constants.ts", "ts.txt"),
        (2, vec!["rev-parse".to_owned()])
    );
    assert_eq!(traced("notes.md", "md.txt"), (0, Vec::new()));
    assert_eq!(traced(draft, "draft.txt"), (0, Vec::new()));
}

fn ignore_file(t: &TempRepo) -> Vec<IgnoreEntry> {
    State::open(&t.path().join(".git"))
        .ignores()
        .expect("read ignore.json")
}

fn fingerprint(report: &Value, rule: &str, file: &str) -> String {
    let finding = findings(report)
        .iter()
        .find(|finding| finding["rule"] == rule && finding["file"] == file);
    let finding = finding.unwrap_or_else(|| panic!("no {rule} in {file}: {report}"));
    finding["fingerprint"]
        .as_str()
        .expect("fingerprint")
        .to_owned()
}

fn rules_of(report: &Value) -> Vec<&str> {
    findings(report)
        .iter()
        .map(|finding| finding["rule"].as_str().expect("rule"))
        .collect()
}

#[test]
fn ignore_writes_an_entry_with_the_file_and_the_rule() {
    let t = branch_repo();
    enable(&t);
    assert_eq!(hook_on(&t, "src/keep.constants.ts").0, 2);
    let args = ["check", "--base", "main", "--json"];
    let fp = fingerprint(
        &check(t.path(), &args, 1),
        "no-any",
        "src/keep.constants.ts",
    );

    let out = run(t.path(), &["ignore", &fp, "--reason", "boundary type"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!("ignored {fp} src/keep.constants.ts\n")
    );
    let [entry] = &ignore_file(&t)[..] else {
        panic!("one entry: {:?}", ignore_file(&t));
    };
    assert_eq!(entry.fingerprint, fp);
    assert_eq!(entry.rule, "no-any");
    assert_eq!(entry.file, "src/keep.constants.ts");
    assert_eq!(entry.reason, "boundary type");
    let added = &entry.added;
    let is_date = added.len() == 10
        && added.char_indices().all(|(i, c)| match i {
            4 | 7 => c == '-',
            _ => c.is_ascii_digit(),
        });
    assert!(is_date && added[..4] >= *"2026", "{added}");

    assert_eq!(rules_of(&check(t.path(), &args, 1)), ["todo-owner"]);
    assert_eq!(hook_on(&t, "src/keep.constants.ts"), (0, String::new()));
}

#[test]
fn ignore_writes_an_entry_per_file_and_replaces_it_again() {
    let t = TempRepo::new();
    t.write("src/m.constants.ts", "export const MAIN = 1;\n")
        .commit("main");
    t.branch("feat")
        .write(
            "src/one.constants.ts",
            "// TODO fix\nexport const ONE = 1;\n",
        )
        .write(
            "src/two.constants.ts",
            "// TODO fix\nexport const TWO = 2;\n",
        )
        .commit("feat");
    let args = ["check", "--base", "main", "--json"];
    let fp = fingerprint(
        &check(t.path(), &args, 1),
        "todo-owner",
        "src/one.constants.ts",
    );
    let entries = |t: &TempRepo| -> Vec<(String, String, String)> {
        ignore_file(t)
            .into_iter()
            .map(|entry| (entry.file.into_string(), entry.rule, entry.reason))
            .collect()
    };
    let expected = |reason: &str| {
        ["src/one.constants.ts", "src/two.constants.ts"]
            .map(|file| (file.into(), "todo-owner".into(), reason.into()))
    };

    let out = run(t.path(), &["ignore", &fp, "--reason", "r"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!("ignored {fp} src/one.constants.ts\nignored {fp} src/two.constants.ts\n")
    );
    assert_eq!(entries(&t), expected("r"));
    assert!(findings(&check(t.path(), &args, 0)).is_empty());

    let out = run(t.path(), &["ignore", &fp, "--reason", "second"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(entries(&t), expected("second"));
}

/// The `any` of `src/old.constants.ts` is inside a function, so its fingerprint differs from the one
/// of `keep.constants.ts`.
fn old_repo() -> TempRepo {
    branch_repo_with(&[(
        "src/old.constants.ts",
        "export function legacy() { const x: any = 1; return x; }\n",
    )])
}

#[test]
fn ignore_of_an_unknown_fingerprint_fails() {
    let t = branch_repo();
    let out = run(t.path(), &["ignore", "no-any:000000", "--reason", "r"]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("no-any:000000"), "{}", stderr(&out));
    assert!(!t.path().join(".git/info/stop-slop/ignore.json").exists());

    let t = old_repo();
    let fp = fingerprint(
        &check(t.path(), &["check", "--all", "--json"], 1),
        "no-any",
        "src/old.constants.ts",
    );
    let out = run(t.path(), &["ignore", &fp, "--reason", "r"]);
    assert_eq!(code(&out), 2, "{}", stdout(&out));
    assert!(stderr(&out).contains(&fp), "{}", stderr(&out));
}

#[test]
fn ignore_needs_a_reason_and_a_repo() {
    let t = branch_repo();
    for args in [
        &["ignore", "no-any:000000"][..],
        &["ignore", "no-any:000000", "--reason", ""],
    ] {
        let out = run(t.path(), args);
        assert_eq!(code(&out), 2, "{args:?}");
        assert!(!stderr(&out).is_empty(), "{args:?}");
    }

    let out = run_outside(&["ignore", "x", "--reason", "r"]);
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).contains("not inside a git repository"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn ignore_with_a_base_checks_the_files_from_it() {
    let t = branch_repo();
    let args = ["check", "--base", "main", "--json"];
    let fp = fingerprint(
        &check(t.path(), &args, 1),
        "no-any",
        "src/keep.constants.ts",
    );
    let out = run(
        t.path(),
        &["ignore", &fp, "--reason", "r", "--base", "main"],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(ignore_file(&t).len(), 1);
    let out = run(
        t.path(),
        &["ignore", &fp, "--reason", "r", "--base", "no-such"],
    );
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).contains("base `no-such` is not a commit"),
        "{}",
        stderr(&out)
    );

    let t = old_repo();
    let fp = fingerprint(
        &check(t.path(), &["check", "--all", "--json"], 1),
        "no-any",
        "src/old.constants.ts",
    );
    let out = run(t.path(), &["ignore", &fp, "--reason", "r"]);
    assert_eq!(code(&out), 2, "{}", stdout(&out));
    let out = run(
        t.path(),
        &["ignore", &fp, "--reason", "r", "--base", "main~1"],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let files: Vec<String> = ignore_file(&t)
        .into_iter()
        .map(|entry| entry.file.into_string())
        .collect();
    assert_eq!(files, ["src/old.constants.ts"]);
}
