use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
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

/// In a fresh temp folder; git stops at its parent, so a repo above it (dotfiles in the home
/// folder) is not found. `repo` passes the variable on to git.
fn run_outside(args: &[&str]) -> Output {
    let dir = tempfile::tempdir().expect("create a temp folder");
    let parent = dir.path().parent().expect("a temp folder has a parent");
    exe(dir.path())
        .args(args)
        .env("GIT_CEILING_DIRECTORIES", parent)
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
    let expected: Vec<_> = ids().into_iter().map(|id| (id, true)).collect();
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
    let expected: Vec<_> = ids().into_iter().map(|id| (id, id != "no-any")).collect();
    assert_eq!(active(&json(&out)), expected);
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

/// `main` with `src/keep.ts`; `feat` changes line 1 of it, keeps the `any` on line 2 and
/// adds `src/new.ts` with a `TODO` without an owner.
fn branch_repo() -> TempRepo {
    let t = TempRepo::new();
    t.write(
        "src/keep.ts",
        "export const a = 1;\nexport const b: any = 2;\n",
    )
    .commit("main");
    t.branch("feat")
        .write(
            "src/keep.ts",
            "export const a = 10;\nexport const b: any = 2;\n",
        )
        .write("src/new.ts", "// TODO fix\nexport const n = 1;\n")
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
    assert_eq!(report["files"], json!(["src/keep.ts", "src/new.ts"]));
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
            "file": "src/keep.ts",
            "line": 2,
            "column": 17,
            "endLine": 2,
            "endColumn": 20,
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
            "file": "src/new.ts",
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
fn check_prints_the_base_and_the_findings_as_text() {
    let t = TempRepo::new();
    t.write("src/m.ts", "export const m = 1;\n").commit("main");
    t.branch("feat")
        .write("src/clean.ts", "export const c = 1;\n")
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
        format!("src/keep.ts:2:17 no-any {}", meta("no-any").message),
        format!("src/new.ts:1:4 todo-owner {}", meta("todo-owner").message),
    ] {
        assert!(lines.contains(&line.as_str()), "no `{line}` in\n{text}");
    }
}

#[test]
fn check_rejects_flags_that_do_not_go_together() {
    let t = branch_repo();
    for args in [
        &["check", "--rev", "feat"][..],
        &["check", "--files", "src/new.ts", "--all"],
        &["check", "--base", "main", "--files", "src/new.ts"],
        &["check", "--base", "main", "--all"],
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
    assert_eq!(report["files"], json!(["src/keep.ts", "src/new.ts"]));
    assert!(!findings(&report).is_empty());
    for finding in findings(&report) {
        assert_eq!(finding["inHunk"], Value::Null, "{finding}");
    }

    let absolute = t.path().join("src/new.ts");
    let src = t.path().join("src");
    for (dir, file) in [
        (t.path(), "src/new.ts"),
        (src.as_path(), "new.ts"),
        (t.path(), absolute.to_str().expect("a UTF-8 temp path")),
    ] {
        let report = check(dir, &["check", "--files", file, "--json"], 1);
        assert_eq!(report["files"], json!(["src/new.ts"]), "{file}");
        assert_eq!(report["base"], Value::Null, "{file}");
        assert_eq!(finding_files(&report), ["src/new.ts"], "{file}");
        assert_eq!(findings(&report)[0]["inHunk"], Value::Null, "{file}");
    }

    t.write("README.md", "# readme\n");
    let args = ["check", "--files", "src/new.ts", "README.md", "--json"];
    assert_eq!(check(t.path(), &args, 1)["files"], json!(["src/new.ts"]));

    t.write("src/gone.ts", "export const g: any = 1;\n")
        .commit("gone");
    fs::remove_file(t.path().join("src/gone.ts")).expect("remove src/gone.ts");
    let report = check(t.path(), &["check", "--all", "--json"], 1);
    assert_eq!(report["files"], json!(["src/keep.ts", "src/new.ts"]));
}

#[test]
fn check_fails_on_a_broken_config_or_ignore_file() {
    let t = branch_repo();
    for (config, key) in [
        ("disabled = [\"no-such\"]\n", "disabled[0]"),
        ("[rules.no-any]\nx = 1\n", "rules.no-any.x"),
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
    t.write("src/m.ts", "export const m = 1;\n").commit("main");
    t.branch("feat")
        .write("src/one.ts", "// TODO fix\nexport const one = 1;\n")
        .write("src/two.ts", "// TODO fix\nexport const two = 2;\n")
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
            file: "src/one.ts".into(),
            reason: "test".into(),
            added: "2026-10-15".into(),
        }])
        .expect("write ignore.json");
    assert_eq!(finding_files(&check(t.path(), &args, 1)), ["src/two.ts"]);

    write_config(&t, "disabled = [\"todo-owner\"]\n");
    assert!(findings(&check(t.path(), &args, 0)).is_empty());
}

#[test]
fn check_at_a_revision_reads_the_files_from_git() {
    let t = TempRepo::new();
    t.write("src/m.ts", "export const m = 1;\n").commit("main");
    t.branch("feat")
        .write("src/r.ts", "export const r: any = 1;\n")
        .commit("feat");
    t.write("src/r.ts", "export const r = 1;\n")
        .write("src/u.ts", "export const u: any = 1;\n");

    let args = ["check", "--rev", "feat", "--base", "main", "--json"];
    let report = check(t.path(), &args, 1);
    assert_eq!(report["files"], json!(["src/r.ts"]));
    assert_eq!(report["base"]["reason"], "flag");
    let [finding] = findings(&report) else {
        panic!("one finding: {report}");
    };
    assert_eq!(finding["rule"], "no-any");
    assert_eq!(finding["file"], "src/r.ts");
    assert_eq!(finding["inHunk"], true);
    let on_disk = fs::read_to_string(t.path().join("src/r.ts")).expect("read src/r.ts");
    assert_eq!(on_disk, "export const r = 1;\n");

    let report = check(t.path(), &["check", "--base", "main", "--json"], 1);
    assert_eq!(finding_files(&report), ["src/u.ts"]);
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
        (&["check", "--files", "src/missing.ts"], "missing.ts"),
    ] {
        let out = run(t.path(), args);
        assert_eq!(code(&out), 2, "{args:?}");
        assert!(stderr(&out).contains(cause), "{args:?}: {}", stderr(&out));
    }
}

#[test]
fn check_reports_a_file_that_does_not_parse() {
    let t = TempRepo::new();
    t.write("src/m.ts", "export const m = 1;\n").commit("main");
    t.branch("feat")
        .write("src/bad.ts", "const = ;\n")
        .commit("feat");
    let report = check(t.path(), &["check", "--base", "main", "--json"], 1);
    let [finding] = findings(&report) else {
        panic!("one finding: {report}");
    };
    assert_eq!(finding["rule"], "parse-error");
    assert_eq!(finding["kind"], "parse-error");
    assert_eq!(finding["file"], "src/bad.ts");
}

#[test]
fn check_skips_files_a_squash_merge_brought_into_the_base() {
    let t = TempRepo::new();
    t.write("shared.ts", "1\n2\n3\n")
        .write("m.ts", "m\n")
        .commit("main");
    t.branch("a")
        .write("a.ts", "a\n")
        .write("shared.ts", "one\n2\n3\n")
        .commit("a");
    t.branch("b")
        .write("b.ts", "b\n")
        .write("shared.ts", "one\n2\nthree\n")
        .commit("b");
    t.checkout("main");
    t.git(&["merge", "-q", "--squash", "a"]);
    t.commit("squash a");
    t.git(&["branch", "-q", "-D", "a"]);
    t.checkout("b");

    let report = check(t.path(), &["check", "--base", "main", "--json"], 0);
    assert_eq!(report["files"], json!(["b.ts", "shared.ts"]));
    assert_eq!(
        report["skipped"],
        json!([{"file": "a.ts", "reason": "already-in-base"}])
    );
}
