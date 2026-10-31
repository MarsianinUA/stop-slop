//! `--before`: the trees, the seal and `--reseal`.

use camino::Utf8Path;
use cli::Exit;
use run::Cancel;
use serde_json::Value;

use super::{World, fake, roomy};

/// The run that `measured-before-seal` of a line names.
fn measured_before_seal(line: &Value) -> Option<&str> {
    let flags = line["flags"].as_array()?;
    let flag = flags
        .iter()
        .find(|flag| flag["kind"] == "measured-before-seal")?;
    flag["run"].as_str()
}

#[test]
fn before_session_right_after_the_snapshot_has_nothing_to_compare() {
    let world = World::new();
    let fake = fake(Vec::new());
    world.run(&fake, &["snapshot"]);
    let runs: [&[&str]; 2] = [
        &["run", "--before", "session", "--json"],
        &["run", "--before", "session", "--dry-run", "--json"],
    ];
    for args in runs {
        let ran = world.run(&fake, args);
        assert_eq!(ran.exit, Exit::Refused, "{args:?}: {}", ran.err);
        assert_eq!(ran.line()["failure"]["phase"], "source", "{args:?}");
        assert!(ran.err.contains("nothing to compare"), "{}", ran.err);
        assert_eq!(ran.next(), ["stop-slop-stand guide"]);
    }
    // The trees come first: a run that cannot compare seals, sweeps and pulls nothing.
    assert!(fake.journal().is_empty(), "{:?}", fake.journal());
    assert!(
        world
            .sessions()
            .iter()
            .all(|session| !session.join("seal").exists())
    );
}

#[test]
fn the_first_run_before_seals_and_a_changed_file_refuses_until_a_reseal() {
    let world = World::new();
    let fake = fake(Vec::new());
    world.run(&fake, &["snapshot"]);
    world.repo.write("main.js", "console.log('v2')\n");
    let first = world.run(&fake, &["run", "--before", "session", "--json"]);
    assert_eq!(first.exit, Exit::Passed, "{}", first.err);
    let journal = first.line()["seal"]["journal"].clone();
    assert!(
        journal.as_array().is_some_and(|entries| entries.len() == 1)
            && journal[0]["sealedAt"].is_string(),
        "{journal}"
    );
    // The run that sealed is not measured before its seal (orchestrator, T-13b): the next run is
    // the second comparison, and no run of this head came before the seal.
    let second = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    assert_eq!(second["comparisons"], 2, "{second}");
    assert_eq!(measured_before_seal(&second), None, "{second}");
    // An edit of the recipe folder refuses with the file and the command that reseals.
    world.write_recipe("load.js", "// load, edited\n");
    let changed = world.run(&fake, &["run", "--before", "session", "--json"]);
    assert_eq!(changed.exit, Exit::Refused, "{}", changed.err);
    let line = changed.line();
    assert_eq!(line["failure"]["phase"], "seal");
    let cause = line["failure"]["cause"].as_str().unwrap();
    assert!(cause.contains("load.js"), "{line}");
    assert!(changed.err.contains("load.js"), "{}", changed.err);
    let reseal = "stop-slop-stand run --before session --reseal --reason \"<text>\"";
    assert_eq!(line["next"], serde_json::json!([reseal]));
    assert_eq!(changed.next(), [reseal]);
    // Without --before the live recipe runs, with no seal in its line.
    let plain = world.run(&fake, &["run", "--json"]);
    assert_eq!(plain.exit, Exit::Passed, "{}", plain.err);
    let plain = plain.line();
    assert!(
        plain["seal"].is_null() && plain["comparisons"].is_null(),
        "{plain}"
    );
    let args = [
        "run",
        "--before",
        "session",
        "--reseal",
        "--reason",
        "a probe of orders",
        "--json",
    ];
    let resealed = world.run(&fake, &args);
    assert_eq!(resealed.exit, Exit::Passed, "{}", resealed.err);
    assert_eq!(resealed.line()["comparisons"], 1, "the count starts again");
    let next = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    let journal = next["seal"]["journal"].to_string();
    assert!(
        journal.contains("resealedAt") && journal.contains("a probe of orders"),
        "{journal}"
    );
    assert_eq!(next["comparisons"], 2, "{next}");
    // The last run of this head before the reseal is the plain one, not the run that resealed.
    assert_eq!(measured_before_seal(&next), plain["run"].as_str(), "{next}");
}

/// A run writes its seal after its pull (batch review 6): a first run the budget refuses leaves
/// none, so the recipe it fixes runs without a reseal.
#[test]
fn a_first_run_before_that_the_budget_refuses_seals_nothing() {
    let world = World::new();
    let fake = fake(Vec::new());
    world.run(&fake, &["snapshot"]);
    world.repo.write("main.js", "console.log('v2')\n");
    let small = |dir: &Utf8Path| {
        let mut host = roomy(dir)?;
        host.cpus = 2;
        Ok(host)
    };
    let args = ["run", "--before", "session", "--json"];
    let refused = world.run_on(&fake, &args, &small, &Cancel::new());
    assert_eq!(refused.exit, Exit::Refused, "{}", refused.err);
    assert_eq!(refused.line()["failure"]["phase"], "budget");
    let sealed = |world: &World| world.sessions().iter().any(|s| s.join("seal").exists());
    assert!(!sealed(&world));
    world.write_recipe("load.js", "// load, smaller\n");
    let ran = world.run(&fake, &args);
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    let journal = ran.line()["seal"]["journal"].clone();
    assert_eq!(journal.as_array().map(Vec::len), Some(1), "{journal}");
    assert!(sealed(&world));
}
