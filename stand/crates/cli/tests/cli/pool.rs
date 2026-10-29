//! Runs of one pool, and the repeats outside the stats a pooled run takes from it.

use std::fs;

use cli::Exit;
use serde_json::Value;

use super::{RECIPE, World, creates, fake, faults, profiled_api, read_json};

#[test]
fn a_second_run_pools_with_the_first_and_a_changed_stand_file_is_recipe_changed() {
    let world = World::new();
    let fake = fake(Vec::new());
    let first = world.run(&fake, &["run", "--json"]).line();
    let second = world.run(&fake, &["run", "--json"]).line();
    assert_eq!(second["attempts"], 2);
    let report = world.report(&second);
    assert_eq!(report["attempts"], 2);
    // The head did not change: the second run takes its setup from the cache.
    assert_eq!(report["head"]["setup"]["cached"], first["run"]);
    assert!(
        report["previousRun"].is_null(),
        "the pool is not the previous run"
    );
    world.write_recipe("compose.yaml", &format!("{RECIPE}# one more line\n"));
    let third = world.run(&fake, &["run", "--json"]).line();
    let report = world.report(&third);
    assert_eq!(report["attempts"], 1);
    assert_eq!(report["previousRun"], second["run"]);
    let flags = report["flags"].to_string();
    assert!(flags.contains("recipe-changed"), "{flags}");
}

#[test]
fn a_pooled_run_takes_coverage_from_the_pool_and_runs_it_again_when_unknown() {
    // A coverage chunk of `api`: the coverage is known, and the second run takes it.
    let world = World::new();
    let chunks = fake(vec![profiled_api()]);
    let first = world.run(&chunks, &["run", "--json"]).line();
    assert!(
        world.report(&first)["coverage"]["from"].is_null(),
        "{}",
        world.report(&first)["coverage"]
    );
    let before = creates(&chunks, "api");
    let second = world.run(&chunks, &["run", "--json"]).line();
    // Two measured repeats of one replica, and no coverage repeat.
    assert_eq!(creates(&chunks, "api") - before, 2);
    let report = world.report(&second);
    assert_eq!(report["coverage"]["from"], first["run"]);
    let run = second["run"].as_str().unwrap();
    assert!(!world.run_dir(run).join("coverage").exists());
    // No chunk: the coverage stays unknown, and every run repeats it.
    let world = World::new();
    let fake = fake(Vec::new());
    let first = world.run(&fake, &["run", "--json"]).line();
    assert!(world.report(&first)["coverage"].is_null());
    let before = creates(&fake, "api");
    let second = world.run(&fake, &["run", "--json"]).line();
    assert_eq!(creates(&fake, "api") - before, 3);
    let run = second["run"].as_str().unwrap();
    assert!(world.run_dir(run).join("coverage").is_dir());
}

#[test]
fn a_pooled_run_takes_the_fault_and_the_second_pass_and_runs_them_again_when_unknown() {
    let recipe = RECIPE.replace("    x-stand: { profile: node }\n", "").replace(
        "  repeat: 2\n",
        "  repeat: 2\n  second_pass: true\n  fault: { target: \"db:5432\", mode: down }\n  \
         expect:\n    unchanged: [{ file: probe.orders.jsonl, reason: the workload only reads orders }]\n",
    );
    let world = World::with(&recipe);
    let fake = fake(Vec::new());
    let first = world.run(&fake, &["run", "--json"]);
    assert_eq!(first.exit, Exit::Passed, "{}", first.err);
    let first = first.line();
    let report = world.report(&first);
    assert!(
        report["fault"]["from"].is_null() && report["secondPass"]["from"].is_null(),
        "{report}"
    );
    assert!(faults(&fake) > 0);
    let mark = faults(&fake);
    let second = world.run(&fake, &["run", "--json"]).line();
    assert_eq!(faults(&fake), mark, "no fault repeat");
    let report = world.report(&second);
    assert_eq!(report["fault"]["from"], first["run"]);
    assert_eq!(report["secondPass"]["from"], first["run"]);
    let dir = world.run_dir(second["run"].as_str().unwrap());
    assert!(!dir.join("pass2-head").exists() && !dir.join("fault-1-head").exists());
    // The last report of the pool knows no category (no head repeat reached the fault) and has no
    // second pass: the next run makes both again.
    let path = dir.join("report.json");
    let mut last = read_json(&path);
    for category in last["fault"]["categories"]["head"].as_array_mut().unwrap() {
        for value in category["values"].as_array_mut().unwrap() {
            *value = Value::String("unknown".into());
        }
    }
    last["secondPass"] = Value::Null;
    fs::write(&path, last.to_string()).unwrap();
    let third = world.run(&fake, &["run", "--json"]).line();
    assert!(faults(&fake) > mark, "the fault repeats again");
    let report = world.report(&third);
    assert!(
        report["fault"]["from"].is_null() && report["secondPass"]["from"].is_null(),
        "{report}"
    );
    assert!(
        world
            .run_dir(third["run"].as_str().unwrap())
            .join("pass2-head")
            .is_dir()
    );
}
