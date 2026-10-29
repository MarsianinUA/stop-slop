//! `run` and its exit codes: the summary, the one `--json` line, the refusals, a failed head,
//! a broken backend and Ctrl+C.

use std::ffi::OsString;
use std::fs;

use camino::Utf8Path;
use cli::Exit;
use run::Cancel;
use sandbox::{Fake, Sandbox, Scenario, Script};
use serde_json::Value;

use super::{
    RECIPE, Ran, SECOND, World, Wrapped, any_create, creates, exits, fake, on_tree, roomy, rule,
};

#[test]
fn a_run_passes_with_its_report_and_leaves_no_tar_and_no_lock() {
    let world = World::new();
    let fake = fake(Vec::new());
    let ran = world.run(&fake, &["run"]);
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    let report = ran
        .out
        .lines()
        .find_map(|line| line.strip_prefix("report "))
        .expect("the path of the report");
    assert!(Utf8Path::new(report).is_file(), "{report}");
    assert!(!report.contains('\\'), "{report}");
    assert!(!world.tar("head").exists() && !world.tar("before").exists());
    assert!(
        world.store.try_lock().unwrap().is_some(),
        "the lock is free"
    );
    assert!(fake.leftovers().unwrap().sandboxes.is_empty());
}

#[test]
fn the_json_line_is_one_line_on_success_failure_and_refusal_and_pretty_indents_it() {
    let world = World::new();
    let passed = world.run(&fake(Vec::new()), &["run", "--json"]).line();
    assert!(
        passed["run"].is_string() && passed["report"].is_string() && passed["failure"].is_null()
    );
    let failed = world.run(
        &fake(vec![rule("api", exits(1, 2 * SECOND))]),
        &["run", "--json"],
    );
    assert_eq!(failed.exit, Exit::AppFailed);
    assert!(failed.line()["report"].is_null() && failed.line()["run"].is_string());
    let refused = World::with("").run(&fake(Vec::new()), &["run", "--json"]);
    assert_eq!(refused.exit, Exit::Refused);
    let line = refused.line();
    assert_eq!(line["failure"]["phase"], "spec");
    assert_eq!(line["next"], serde_json::json!(["stop-slop-stand init"]));
    assert_eq!(refused.next(), ["stop-slop-stand init"]);
    // A flag `clap` refuses: the line of the phase `args`, as `check --json` of the linter.
    let flag = world.run(&fake(Vec::new()), &["run", "--json", "--fast"]);
    assert_eq!(flag.exit, Exit::Refused);
    let line = flag.line();
    assert_eq!(line["failure"]["phase"], "args");
    assert!(
        line["failure"]["cause"]
            .as_str()
            .unwrap()
            .contains("--fast"),
        "{line}"
    );
    assert_eq!(line["next"], serde_json::json!(["stop-slop-stand guide"]));
    assert_eq!(flag.next(), ["stop-slop-stand guide"]);
    let pretty = world.run(&fake(Vec::new()), &["run", "--json", "--pretty"]);
    let value: Value = serde_json::from_str(&pretty.out).unwrap();
    assert!(pretty.out.lines().count() > 1 && value["run"].is_string());
    // Before `execute`: the folders of the OS, the stand folder or the adapter fail in `main`.
    for (args, json) in [
        (&["run", "--json"][..], true),
        (&["run", "--dry-run", "--json"], true),
        (&["run"], false),
        (&["guide"], false),
    ] {
        let args = std::iter::once("stop-slop-stand").chain(args.iter().copied());
        let command = cli::parse(args.map(OsString::from)).unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let exit = cli::refuse_start(&command, "C:/stand: denied\nby the OS", &mut out, &mut err);
        let ran = Ran::new(exit, out, err);
        assert_eq!(ran.exit, Exit::Broken);
        assert!(
            ran.err.starts_with("C:/stand: denied\nby the OS\n"),
            "{}",
            ran.err
        );
        assert_eq!(ran.next(), ["stop-slop-stand guide"]);
        if json {
            let line = ran.line();
            assert_eq!(line["failure"]["phase"], "config");
            assert_eq!(line["failure"]["cause"], "C:/stand: denied; by the OS");
            assert_eq!(line["next"], serde_json::json!(["stop-slop-stand guide"]));
        } else {
            assert_eq!(ran.out, "");
        }
    }
}

#[test]
fn refusals_before_the_start_are_2_with_their_next_and_no_vm() {
    let wide = RECIPE.replace("cpus: \"1\", memory: 128m", "cpus: \"1000\", memory: 128m");
    assert_ne!(wide, RECIPE);
    let missing = || {
        Fake::new(Scenario {
            missing_images: vec!["postgres:17".into()],
            ..Scenario::default()
        })
    };
    let cases: Vec<(World, &[&str], Fake, &str, &str)> = vec![
        (
            World::with(""),
            &[],
            fake(Vec::new()),
            "spec",
            "stop-slop-stand init",
        ),
        (
            World::with("services: {}\n"),
            &[],
            fake(Vec::new()),
            "spec",
            "stop-slop-stand guide recipe",
        ),
        (
            World::with(&wide),
            &[],
            fake(Vec::new()),
            "budget",
            "stop-slop-stand guide",
        ),
        (
            World::new(),
            &["--before", "session"],
            fake(Vec::new()),
            "source",
            "stop-slop-stand snapshot",
        ),
        (
            World::new(),
            &["--before", "no-such-ref"],
            fake(Vec::new()),
            "source",
            "stop-slop-stand guide",
        ),
        (
            World::new(),
            &[],
            missing(),
            "pull",
            "stop-slop-stand guide",
        ),
    ];
    for (world, extra, fake, phase, next) in cases {
        let mut args = vec!["run"];
        args.extend(extra);
        args.push("--json");
        let ran = world.run(&fake, &args);
        assert_eq!(ran.exit, Exit::Refused, "{args:?}: {}", ran.err);
        let line = ran.line();
        assert_eq!(line["failure"]["phase"], phase, "{line}");
        assert!(line["run"].is_null(), "{line}");
        assert_eq!(ran.next(), [next], "{args:?}");
        assert!(!any_create(&fake), "{args:?}");
    }
}

#[test]
fn a_failed_head_is_1_without_a_report_and_names_inspect_or_the_recipe() {
    let world = World::new();
    // `api` dies in the head's first repeat: `db` keeps its failure snapshot.
    let died = fake(vec![on_tree(
        "api",
        world.tar("head"),
        exits(1, 2 * SECOND),
    )]);
    let ran = world.run(&died, &["run", "--json"]);
    assert_eq!(ran.exit, Exit::AppFailed, "{}", ran.err);
    let line = ran.line();
    let run = line["run"].as_str().unwrap();
    assert_eq!(
        line["next"],
        serde_json::json!([format!("stop-slop-stand inspect {run} head db -- <argv>")])
    );
    assert!(line["report"].is_null());
    assert!(!world.run_dir(run).join("report.json").exists());
    assert!(
        ran.err
            .starts_with("the head failed: workload died in api, repeat 1; logs "),
        "{}",
        ran.err
    );
    // Setup fails: nothing is kept, so the next step is the rules of the recipe.
    let world = World::new();
    let setup = fake(vec![on_tree(
        "prepare",
        world.tar("head"),
        exits(1, SECOND),
    )]);
    let ran = world.run(&setup, &["run"]);
    assert_eq!(ran.exit, Exit::AppFailed, "{}", ran.err);
    assert!(
        ran.out.ends_with("next: stop-slop-stand guide recipe\n"),
        "{}",
        ran.out
    );
    let line = world.run(&setup, &["run", "--json"]).line();
    assert_eq!(line["failure"]["phase"], "setup");
    assert_eq!(
        line["next"],
        serde_json::json!(["stop-slop-stand guide recipe"])
    );
}

#[test]
fn a_broken_backend_is_3_and_ctrl_c_is_130() {
    let world = World::new();
    let base = fake(Vec::new());
    let broken = Wrapped {
        fails_on: Some("prepare"),
        ..Wrapped::new(&base)
    };
    let ran = world.run(&broken, &["run", "--json"]);
    assert_eq!(ran.exit, Exit::Broken, "{}", ran.err);
    assert_eq!(ran.line()["failure"]["phase"], "setup");
    assert!(ran.line()["run"].is_string());
    assert_eq!(ran.next(), ["stop-slop-stand guide"]);
    let cancel = Cancel::new();
    let base = fake(Vec::new());
    let hooked = Wrapped {
        cancels_on: Some(("api", &cancel)),
        ..Wrapped::new(&base)
    };
    let ran = world.run_on(&hooked, &["run", "--json"], &roomy, &cancel);
    assert_eq!(ran.exit, Exit::Cancelled, "{}", ran.err);
    assert_eq!(ran.line()["failure"]["cause"], "cancelled");
    assert!(ran.next().is_empty(), "{}", ran.err);
    assert!(
        base.leftovers().unwrap().sandboxes.is_empty(),
        "every VM destroyed"
    );
    let run = ran.line()["run"].as_str().unwrap().to_owned();
    assert!(world.run_dir(&run).is_dir());
    assert!(!world.tar("head").exists());
}

/// The guest wrote a result the stand will not read: the recipe's fault, not the stand's, whether
/// the setup cache or the report meets it (batch review 6).
#[test]
fn a_result_that_is_not_a_plain_file_or_over_the_limit_is_2_with_the_recipe() {
    let refused = |ran: &Ran, phase: &str, file: &str| {
        assert_eq!(ran.exit, Exit::Refused, "{}", ran.err);
        let line = ran.line();
        assert_eq!(line["failure"]["phase"], phase, "{line}");
        assert!(line["run"].is_string(), "{line}");
        assert!(ran.err.contains(file), "{}", ran.err);
        assert_eq!(ran.next(), ["stop-slop-stand guide recipe"]);
    };
    // The report meets a folder the workload left.
    let writes = vec![("/stand/out/odd.jsonl/inner.jsonl".into(), b"{}\n".to_vec())];
    let load = Script {
        writes,
        ..exits(0, 10 * SECOND)
    };
    let ran = World::new().run(&fake(vec![rule("load", load)]), &["run", "--json"]);
    refused(&ran, "report", "odd.jsonl");

    // The setup cache meets an `after-setup` output over the limit, before any repeat: `prepare`
    // waits for `db`, so `db` runs in setup with its probe.
    let npm = r#"    command: ["npm", "ci", "--cache", "/w/.npm"]"#;
    let waits = format!("{npm}\n    depends_on: {{ db: {{ condition: service_healthy }} }}");
    let probe = "  probes:\n    schema: { in: db, at: after-setup, run: [psql] }\n";
    let probed = RECIPE.replace(npm, &waits).replace("  probes:\n", probe);
    let base = fake(Vec::new());
    let grow = |stdout: &Utf8Path| {
        let file = fs::OpenOptions::new().write(true).open(stdout).unwrap();
        file.set_len(report::RESULT_LIMIT + 1).unwrap();
    };
    let large = Wrapped {
        after_exec: Some(&grow),
        ..Wrapped::new(&base)
    };
    let ran = World::with(&probed).run(&large, &["run", "--json"]);
    refused(&ran, "setup", "probe.schema.jsonl");
    assert!(ran.err.contains("64 MiB"), "{}", ran.err);
    assert_eq!(creates(&base, "load"), 0);
}

#[test]
fn a_bad_stand_toml_is_2_with_its_file_and_key() {
    let cases = [
        ("host_share = 90\n", "host_share"),
        ("max_tolerance = 2000\n", "max_tolerance"),
        ("x = [\n", "toml"),
    ];
    for (text, key) in cases {
        let world = World::new();
        fs::write(&world.paths.config, text).unwrap();
        let fake = fake(Vec::new());
        let ran = world.run(&fake, &["run", "--json"]);
        assert_eq!(ran.exit, Exit::Refused, "{text}");
        assert_eq!(ran.line()["failure"]["phase"], "config");
        let said = format!("{}: {key}: ", world.paths.config);
        assert!(ran.err.contains(&said), "{said} in {}", ran.err);
        assert!(!any_create(&fake));
    }
}

#[test]
fn a_stand_run_outside_git_is_2() {
    let world = World::new();
    let outside = tempfile::tempdir().unwrap();
    let cwd = Utf8Path::from_path(outside.path()).unwrap();
    let fake = fake(Vec::new());
    let ran = world.run_at(cwd, &fake, &["run", "--json"], &roomy, &Cancel::new());
    assert_eq!(ran.exit, Exit::Refused);
    assert_eq!(ran.line()["failure"]["phase"], "source");
    assert!(
        ran.err.contains("not inside a git work tree"),
        "{}",
        ran.err
    );
    assert!(!any_create(&fake));
}
