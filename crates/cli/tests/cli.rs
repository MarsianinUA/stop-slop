use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use kit::RuleMeta;
use rules::RULESET;
use serde_json::Value;
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
