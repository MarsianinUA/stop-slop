use std::collections::BTreeSet;
use std::process::{Command, Stdio};

use serde_json::Value;
use testkit::TempRepo;

const SKILL: &str = include_str!("../../../skill/SKILL.md");

/// The names in backticks after `prefix` on its line of `SKILL.md`, up to the first `.` or `:`
/// outside backticks. Text in parentheses describes the name before it, such as what `files`
/// holds, and is skipped.
fn listed(prefix: &str) -> BTreeSet<&'static str> {
    let rest = SKILL
        .lines()
        .find_map(|line| line.trim_start().strip_prefix(prefix))
        .unwrap_or_else(|| panic!("no line of skill/SKILL.md starts with {prefix}"));
    let mut names = BTreeSet::new();
    let mut depth = 0_u32;
    // Even pieces lie outside backticks, odd ones inside.
    for (i, piece) in rest.split('`').enumerate() {
        if i % 2 == 1 {
            if depth == 0 {
                names.insert(piece);
            }
            continue;
        }
        for mark in piece.chars() {
            match mark {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                '.' | ':' if depth == 0 => return names,
                _ => {}
            }
        }
    }
    names
}

fn keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("an object: {value}"))
        .keys()
        .map(String::as_str)
        .collect()
}

fn check(t: &TempRepo, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_stop-slop"));
    command
        .arg("check")
        .args(args)
        .arg("--json")
        .current_dir(t.path())
        .stdin(Stdio::null());
    command
}

fn json(command: &mut Command, exit: i32) -> Value {
    let out = command.output().expect("start stop-slop");
    assert_eq!(
        out.status.code(),
        Some(exit),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("JSON on stdout")
}

#[test]
fn skill_md_lists_the_fields_of_check_json() {
    let t = TempRepo::new();
    t.write("src/a.constants.ts", "export const A = 1;\n")
        .commit("main");
    t.branch("feat")
        .write("src/a.constants.ts", "export const A: any = 1;\n");
    let report = json(&mut check(&t, &["--base", "main"]), 1);

    assert_eq!(keys(&report), listed("- `stop-slop check --json` prints"));
    assert_eq!(keys(&report["base"]), listed("- `base` has"));
    assert_eq!(keys(&report["findings"][0]), listed("- A finding has"));
}

/// `Command` looks only for `gh.exe` on Windows, so a script stands in for `gh` on Linux and
/// macOS only.
#[cfg(unix)]
#[test]
fn skill_md_lists_the_fields_of_pr() {
    use std::os::unix::fs::PermissionsExt;
    use std::{env, fs, iter};

    let t = TempRepo::new();
    t.write("src/a.constants.ts", "export const A = 1;\n")
        .commit("main");
    // The repository is its own `origin`, with the pull request at its head.
    t.git(&["update-ref", "refs/pull/1/head", "HEAD"]);
    t.git(&["remote", "add", "origin", "."]);
    let head = t.git(&["rev-parse", "HEAD"]);
    let bin = tempfile::tempdir().expect("create a temp folder");
    let gh = bin.path().join("gh");
    fs::write(
        &gh,
        format!("#!/bin/sh\nprintf '%s\\n' {head} main title\n"),
    )
    .expect("write gh");
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).expect("make gh runnable");
    let path = env::var_os("PATH").unwrap_or_default();
    let path = env::join_paths(iter::once(bin.path().to_owned()).chain(env::split_paths(&path)))
        .expect("a PATH");
    let report = json(check(&t, &["--pr", "1"]).env("PATH", path), 0);

    assert_eq!(keys(&report["pr"]), listed("- `pr` has"));
}
