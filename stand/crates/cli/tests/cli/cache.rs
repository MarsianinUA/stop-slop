//! The setup cache: its slots, its entries and what a run that halts drops.

use camino::{Utf8Path, Utf8PathBuf};
use cli::Exit;
use run::Cancel;
use sandbox::{Sandbox, Script};

use super::{
    SECOND, World, Wrapped, cached_of, creates, entries, exits, fake, folders, healthy, on_tree,
    read_json, roomy, set, snapshots_left,
};

#[test]
fn a_failed_repeat_keeps_the_slot_and_a_breakage_or_ctrl_c_drops_what_the_run_built() {
    let world = World::new();
    // The head's setup passes, then `api` dies: exit 1, and the entry stays for the next run.
    let died = fake(vec![on_tree(
        "api",
        world.tar("head"),
        Script {
            exit: Some((1, 5 * SECOND)),
            ..healthy()
        },
    )]);
    assert_eq!(world.run(&died, &["run"]).exit, Exit::AppFailed);
    let prepares = creates(&died, "prepare");
    assert_eq!(world.run(&died, &["run"]).exit, Exit::AppFailed);
    assert_eq!(
        creates(&died, "prepare"),
        prepares,
        "the head's setup comes from the entry"
    );
    // A breakage after the setup of a new head tree: the slot stays as it was, and what this run
    // built goes.
    let world = World::new();
    let base = fake(Vec::new());
    assert_eq!(world.run(&base, &["run"]).exit, Exit::Passed);
    let (entries_then, left_then) = (entries(&world), set(&snapshots_left(&base)));
    world.repo.write("main.js", "console.log('v2')\n");
    let broken = Wrapped {
        fails_on: Some("api"),
        ..Wrapped::new(&base)
    };
    let prepares = creates(&base, "prepare");
    assert_eq!(world.run(&broken, &["run"]).exit, Exit::Broken);
    assert_eq!(creates(&base, "prepare") - prepares, 1, "its setup ran");
    assert_eq!(entries(&world), entries_then);
    assert_eq!(set(&snapshots_left(&base)), left_then);
    // It kept nothing, so the kept run is still the first.
    let session = world.sessions().remove(0);
    let first = folders(&session.join("runs"))[0].clone();
    let state = read_json(&session.join("session.json"));
    assert_eq!(state["kept"], first.file_name().unwrap());
    // Ctrl+C after setup: nothing of the cancelled run stays.
    let world = World::new();
    let base = fake(Vec::new());
    let cancel = Cancel::new();
    let hooked = Wrapped {
        cancels_on: Some(("api", &cancel)),
        ..Wrapped::new(&base)
    };
    assert_eq!(
        world.run_on(&hooked, &["run"], &roomy, &cancel).exit,
        Exit::Cancelled
    );
    assert!(creates(&base, "prepare") > 0, "its setup ran");
    assert!(
        entries(&world).is_empty(),
        "no entry of the cancelled run stays"
    );
    assert!(snapshots_left(&base).is_empty(), "nor its snapshots");
}

/// Each step of settling on its own (batch review 6): a cache that fails to settle costs neither
/// the kept run nor the tar files, and the head's failure still shows.
#[test]
fn a_cache_that_fails_to_settle_still_keeps_the_run_and_clears_the_trees() {
    let world = World::new();
    let died = fake(vec![on_tree(
        "api",
        world.tar("head"),
        exits(1, 2 * SECOND),
    )]);
    assert_eq!(world.run(&died, &["run"]).exit, Exit::AppFailed);
    // A new head tree: its entry replaces the first one, whose snapshots will not drop.
    let session = world.sessions().remove(0);
    let first = cached_of(&session);
    world.repo.write("main.js", "console.log('v2')\n");
    let broken = Wrapped {
        undroppable: &first,
        ..Wrapped::new(&died)
    };
    let ran = world.run(&broken, &["run", "--json"]);
    assert_eq!(ran.exit, Exit::Broken, "{}", ran.err);
    let line = ran.line();
    assert_eq!(line["failure"]["phase"], "report");
    assert!(
        ran.err.contains("the head failed: workload died in api"),
        "{}",
        ran.err
    );
    let state = read_json(&session.join("session.json"));
    assert_eq!(state["kept"], line["run"]);
    assert!(!world.tar("head").exists());
    let args = [
        "inspect",
        line["run"].as_str().unwrap(),
        "head",
        "db",
        "--",
        "true",
    ];
    let looked = world.run(&died, &args);
    assert_eq!(looked.exit, Exit::Passed, "{}", looked.err);
}

/// The before slot lets go of an entry the head takes in the same run: it keeps its snapshots
/// (batch review 6).
#[test]
fn a_run_with_the_two_trees_swapped_takes_both_setups_and_keeps_both_entries() {
    let world = World::new();
    let fake = fake(Vec::new());
    world.run(&fake, &["snapshot"]);
    world.repo.write("main.js", "console.log('v2')\n");
    let first = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    // The snapshot takes the new tree, and the work tree goes back to the old one.
    world.run(&fake, &["snapshot"]);
    world.repo.write("main.js", "console.log('v1')\n");
    let prepares = creates(&fake, "prepare");
    let swapped = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    assert_eq!(creates(&fake, "prepare"), prepares, "both from the cache");
    let report = world.report(&swapped);
    assert_eq!(report["before"]["setup"]["cached"], first["run"]);
    assert_eq!(report["head"]["setup"]["cached"], first["run"]);
    let session = world.sessions().remove(0);
    assert!(set(&cached_of(&session)).is_subset(&set(&snapshots_left(&fake))));
    let again = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    assert_eq!(
        creates(&fake, "prepare"),
        prepares,
        "both from the cache again"
    );
    assert_eq!(
        world.report(&again)["head"]["setup"]["cached"],
        first["run"]
    );
}

/// The folder of the entry a cache slot of a session names (`cache.<slot>.digest` of
/// `session.json`, a law of the blueprint).
fn slot_entry(session: &Utf8Path, slot: &str) -> Utf8PathBuf {
    let state = read_json(&session.join("session.json"));
    let digest = state["cache"][slot]["digest"]
        .as_str()
        .unwrap_or_else(|| panic!("no {slot} slot: {state}"));
    session.join("cache").join(digest)
}

#[test]
fn a_second_run_before_session_takes_the_before_setup_from_the_cache() {
    let world = World::new();
    let fake = fake(Vec::new());
    world.run(&fake, &["snapshot"]);
    world.repo.write("main.js", "console.log('v2')\n");
    let first = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    let report = world.report(&first);
    assert!(
        report["before"]["setup"]["cached"].is_null()
            && report["head"]["setup"]["cached"].is_null()
    );
    // No edit: both setups come from the entries of the first run.
    let prepares = creates(&fake, "prepare");
    let second = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    assert_eq!(creates(&fake, "prepare"), prepares, "no setup runs");
    let report = world.report(&second);
    assert_eq!(report["before"]["setup"]["cached"], first["run"]);
    assert_eq!(report["head"]["setup"]["cached"], first["run"]);
    // Another head tree: only the head sets up again.
    world.repo.write("main.js", "console.log('v3')\n");
    let prepares = creates(&fake, "prepare");
    let third = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    assert_eq!(creates(&fake, "prepare") - prepares, 1);
    let report = world.report(&third);
    assert_eq!(report["before"]["setup"]["cached"], first["run"]);
    assert!(report["head"]["setup"]["cached"].is_null());
    // Another digest of an image misses for both versions.
    let moved = Wrapped {
        digests: &[("postgres:17", "sha256:moved")],
        ..Wrapped::new(&fake)
    };
    let prepares = creates(&fake, "prepare");
    let fourth = world
        .run(&moved, &["run", "--before", "session", "--json"])
        .line();
    assert_eq!(creates(&fake, "prepare") - prepares, 2);
    assert!(world.report(&fourth)["before"]["setup"]["cached"].is_null());
    // A reseal puts another recipe digest in the key: a miss again.
    world.write_recipe("load.js", "// load, edited\n");
    let prepares = creates(&fake, "prepare");
    let args = [
        "run",
        "--before",
        "session",
        "--reseal",
        "--reason",
        "edited load",
        "--json",
    ];
    let fifth = world.run(&moved, &args);
    assert_eq!(fifth.exit, Exit::Passed, "{}", fifth.err);
    assert_eq!(creates(&fake, "prepare") - prepares, 2);
}

#[test]
fn the_slots_follow_the_last_run_and_a_head_entry_serves_the_next_before() {
    let world = World::new();
    let fake = fake(Vec::new());
    world.run(&fake, &["snapshot"]);
    // Without --before the head's entry fills the head slot; its tree is the snapshot's.
    let alone = world.run(&fake, &["run", "--json"]).line();
    world.repo.write("main.js", "console.log('v2')\n");
    let prepares = creates(&fake, "prepare");
    let compared = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    // "Before" hits the entry the head built a run ago; only the new head sets up.
    assert_eq!(creates(&fake, "prepare") - prepares, 1);
    assert_eq!(
        world.report(&compared)["before"]["setup"]["cached"],
        alone["run"]
    );
    let session = world.sessions().remove(0);
    let head_entry = slot_entry(&session, "head");
    let head_ids = run::cached_ids(&head_entry).unwrap().unwrap();
    // A run without --before changes only the head slot: the entry it replaces goes, snapshots
    // and folder, and "before" still hits after it.
    world.repo.write("main.js", "console.log('v3')\n");
    let plain = world.run(&fake, &["run", "--json"]).line();
    assert!(!head_entry.exists());
    assert!(set(&head_ids).is_disjoint(&set(&snapshots_left(&fake))));
    let prepares = creates(&fake, "prepare");
    let next = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    assert_eq!(creates(&fake, "prepare"), prepares, "both from the cache");
    let report = world.report(&next);
    assert_eq!(report["before"]["setup"]["cached"], alone["run"]);
    assert_eq!(report["head"]["setup"]["cached"], plain["run"]);
    // An entry with a snapshot gone from the home is a miss: only "before" sets up again.
    let before_ids = run::cached_ids(&slot_entry(&session, "before"))
        .unwrap()
        .unwrap();
    fake.drop_snapshot(&before_ids[0]).unwrap();
    let dry = world.run(&fake, &["run", "--before", "session", "--dry-run"]);
    let plain_run = plain["run"].as_str().unwrap();
    let said = format!("\nsetup head cached {plain_run}\nsetup before built\n");
    assert!(dry.out.ends_with(&said), "{}", dry.out);
    let prepares = creates(&fake, "prepare");
    let missed = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    assert_eq!(creates(&fake, "prepare") - prepares, 1);
    let report = world.report(&missed);
    assert!(report["before"]["setup"]["cached"].is_null());
    assert_eq!(report["head"]["setup"]["cached"], plain["run"]);
}
