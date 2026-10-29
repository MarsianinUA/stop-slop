//! `inspect` of a kept run.

use std::fs;

use camino::Utf8Path;
use cli::Exit;
use run::Cancel;
use sandbox::{Call, ExecOutcome, Rule, Sandbox, Script};

use super::{World, Wrapped, fake, healthy, looked, roomy, rule, set, snapshots_left};

/// `db` whose first `inspect` output, `1.out`, is 50 rows with the code 3.
fn inspected_db() -> Rule {
    let rows: String = (1..=50).map(|n| format!("row {n}\n")).collect();
    rule(
        "db",
        Script {
            volume_bytes: 4096,
            probes: vec![
                (
                    "probe.orders.jsonl".into(),
                    ExecOutcome::Exited(0),
                    b"{\"id\":1}\n".to_vec(),
                ),
                ("1.out".into(), ExecOutcome::Exited(3), rows.into_bytes()),
            ],
            ..healthy()
        },
    )
}

#[test]
fn inspect_prints_the_code_forty_lines_and_the_path_and_refuses_what_is_not_kept() {
    let world = World::new();
    let fake = fake(vec![inspected_db()]);
    let line = world.run(&fake, &["run", "--json"]).line();
    let run = line["run"].as_str().unwrap();
    let snapshots = set(&snapshots_left(&fake));
    let args = ["inspect", run, "head", "db", "--", "psql", "-c", "select 1"];
    let ran = world.run(&fake, &args);
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    let lines: Vec<&str> = ran.out.lines().collect();
    assert_eq!(lines.len(), 1 + 40 + 1, "{}", ran.out);
    assert_eq!(
        (lines[0], lines[1], lines[40]),
        ("exit 3", "row 1", "row 40")
    );
    assert_eq!(lines[41], format!("full output {}", looked(&world, run, 1)));
    let argv = fake
        .journal()
        .into_iter()
        .rev()
        .find_map(|call| match call {
            Call::Exec { argv, .. } => Some(argv),
            _ => None,
        });
    assert_eq!(argv.unwrap(), ["psql", "-c", "select 1"]);
    // The copy is gone and the snapshots are as they were.
    assert!(fake.leftovers().unwrap().sandboxes.is_empty());
    assert_eq!(set(&snapshots_left(&fake)), snapshots);
    let again = world.run(&fake, &["inspect", run, "head", "db", "--", "true"]);
    let said = format!("exit 0\nfull output {}\n", looked(&world, run, 2));
    assert_eq!(again.out, said);
    // One line of a megabyte: 64 KiB of it are read and printed.
    let long = |stdout: &Utf8Path| {
        fs::write(stdout, format!("{}\nrow 2\n", "x".repeat(1 << 20))).unwrap();
    };
    let swapped = Wrapped {
        after_exec: Some(&long),
        ..Wrapped::new(&fake)
    };
    let ran = world.run(&swapped, &["inspect", run, "head", "db", "--", "true"]);
    let lines: Vec<&str> = ran.out.lines().collect();
    assert_eq!(lines.len(), 3, "{}", ran.err);
    assert_eq!(lines[1], "x".repeat(64 << 10));
    assert_eq!(lines[2], format!("full output {}", looked(&world, run, 3)));
    // What `argv` leaves in place of its output is not a regular file: nothing of it is printed.
    let folder = |stdout: &Utf8Path| {
        fs::remove_file(stdout).unwrap();
        fs::create_dir(stdout).unwrap();
    };
    let swapped = Wrapped {
        after_exec: Some(&folder),
        ..Wrapped::new(&fake)
    };
    let ran = world.run(&swapped, &["inspect", run, "head", "db", "--", "true"]);
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    let not_regular = |n| {
        let path = looked(&world, run, n);
        format!("exit 0\nthe output {path} is not a regular file; nothing of it is printed\n")
    };
    assert_eq!(ran.out, not_regular(4));
    // Windows makes a link only in developer mode or for an administrator, so Linux CI runs this.
    #[cfg(unix)]
    {
        let secret = world.paths.root.join("secret");
        fs::write(&secret, "a file of the host\n").unwrap();
        let link = |stdout: &Utf8Path| {
            fs::remove_file(stdout).unwrap();
            std::os::unix::fs::symlink(&secret, stdout).unwrap();
        };
        let linked = Wrapped {
            after_exec: Some(&link),
            ..Wrapped::new(&fake)
        };
        let ran = world.run(&linked, &["inspect", run, "head", "db", "--", "true"]);
        assert_eq!(ran.out, not_regular(5));
    }
    // Another run, a side or a service the kept run has no snapshot of: 2, and no VM.
    let mark = fake.journal().len();
    let refusals: [(&[&str], String); 3] = [
        (
            &["inspect", "20200101-000000", "head", "db", "--", "true"],
            format!("which keeps {run}"),
        ),
        (
            &["inspect", run, "before", "db", "--", "true"],
            "on the before side".to_owned(),
        ),
        (
            &["inspect", run, "head", "api", "--", "true"],
            "`api`".to_owned(),
        ),
    ];
    for (args, said) in refusals {
        let ran = world.run(&fake, args);
        assert_eq!(ran.exit, Exit::Refused, "{args:?}");
        assert!(ran.err.contains(&said), "{said} in {}", ran.err);
        assert_eq!(ran.next(), ["stop-slop-stand guide inspect"], "{args:?}");
    }
    let created = |from: usize| {
        let journal = fake.journal();
        journal[from..]
            .iter()
            .any(|call| matches!(call, Call::Create { .. }))
    };
    assert!(!created(mark));
    // Ctrl+C before the copy starts: 130 and no VM.
    let cancel = Cancel::new();
    cancel.cancel();
    let args = ["inspect", run, "head", "db", "--", "true"];
    let ran = world.run_on(&fake, &args, &roomy, &cancel);
    assert_eq!(ran.exit, Exit::Cancelled, "{}", ran.err);
    assert!(ran.next().is_empty(), "{}", ran.err);
    assert!(!created(mark));
    // A `kept.json` that does not read breaks the stand.
    fs::write(world.run_dir(run).join("kept.json"), "[").unwrap();
    let ran = world.run(&fake, &args);
    assert_eq!(ran.exit, Exit::Broken, "{}", ran.err);
    assert_eq!(ran.next(), ["stop-slop-stand guide"]);
    // The session is the stand file's: without one there is no session to inspect.
    fs::remove_file(world.recipe().join("compose.yaml")).unwrap();
    let ran = world.run(&fake, &args);
    assert_eq!(ran.exit, Exit::Refused, "{}", ran.err);
    assert_eq!(ran.next(), ["stop-slop-stand init"]);
}

#[test]
fn inspect_of_a_service_that_does_not_start_alone_is_2() {
    // `db` never gets healthy: the head fails and keeps it, and its copy does not start either.
    let world = World::new();
    let unhealthy = Script {
        volume_bytes: 4096,
        health: vec![Some(1)],
        ..healthy()
    };
    let fake = fake(vec![rule("db", unhealthy)]);
    let failed = world.run(&fake, &["run", "--json"]);
    assert_eq!(failed.exit, Exit::AppFailed, "{}", failed.err);
    let run = failed.line()["run"].as_str().unwrap().to_owned();
    let ran = world.run(&fake, &["inspect", &run, "head", "db", "--", "true"]);
    assert_eq!(ran.exit, Exit::Refused, "{}", ran.err);
    assert!(
        ran.err
            .starts_with("db did not start in inspect: it stayed unhealthy"),
        "{}",
        ran.err
    );
    assert_eq!(ran.next(), ["stop-slop-stand guide inspect"]);
    assert!(fake.leftovers().unwrap().sandboxes.is_empty());
}
