//! `run --dry-run`: the checks of `run`, with nothing pulled, sealed, built or dropped.

use camino::Utf8Path;
use cli::Exit;
use run::Cancel;
use sandbox::Call;

use super::{World, Wrapped, fake, folders, roomy};

#[test]
fn dry_run_creates_nothing_pulls_nothing_never_seals_and_names_the_cache() {
    let world = World::new();
    let fake = fake(Vec::new());
    world.run(&fake, &["snapshot"]);
    world.repo.write("main.js", "console.log('v2')\n");
    let ran = world.run(&fake, &["run", "--before", "session", "--dry-run"]);
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    let first = ran.out.lines().next().unwrap();
    assert!(
        first.starts_with("dry run: ") && first.contains("compose.yaml (live folder), recipe "),
        "{}",
        ran.out
    );
    assert!(!first.contains('\\'), "{first}");
    assert!(
        ran.out.lines().nth(1).unwrap().starts_with("budget: "),
        "{}",
        ran.out
    );
    assert!(
        ran.out
            .contains("\nimage postgres:17 cached fake:postgres:17\n")
            && ran.out.contains("\nimage node:22 cached fake:node:22\n"),
        "{}",
        ran.out
    );
    assert!(
        ran.out
            .ends_with("\nsetup head built\nsetup before built\n"),
        "{}",
        ran.out
    );
    let json = world.run(
        &fake,
        &["run", "--before", "session", "--dry-run", "--json"],
    );
    assert_eq!(json.exit, Exit::Passed, "{}", json.err);
    let line = json.line();
    assert!(
        line["run"].is_null() && line["report"].is_null() && line["failure"].is_null(),
        "{line}"
    );
    let made = |call: &Call| {
        matches!(
            call,
            Call::Create { .. } | Call::Pull(_) | Call::DropSnapshot(_)
        )
    };
    assert!(!fake.journal().iter().any(made), "{:?}", fake.journal());
    assert!(
        world.sessions().iter().all(|session| {
            !session.join("seal").exists() && folders(&session.join("runs")).is_empty()
        }),
        "no seal and no run folder"
    );
    // After a run, the dry run reads the seal and names the run whose setups it would take.
    let line = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    let run = line["run"].as_str().unwrap();
    let mark = fake.journal().len();
    let again = world.run(&fake, &["run", "--before", "session", "--dry-run"]);
    assert!(
        again.out.contains("compose.yaml (sealed copy of "),
        "{}",
        again.out
    );
    let both = format!("\nsetup head cached {run}\nsetup before cached {run}\n");
    assert!(again.out.ends_with(&both), "{}", again.out);
    assert!(!fake.journal()[mark..].iter().any(made), "it drops nothing");
    // An image gone from the home would download, so no setup can come from the cache.
    let evicted = Wrapped {
        evicted: &["node:22"],
        ..Wrapped::new(&fake)
    };
    let gone = world.run(&evicted, &["run", "--before", "session", "--dry-run"]);
    assert!(
        gone.out.contains("\nimage node:22 to download\n")
            && gone
                .out
                .ends_with("\nsetup head built\nsetup before built\n"),
        "{}",
        gone.out
    );
    // The refusals of `run` are its refusals: an edit of the sealed recipe, a stand too big.
    world.write_recipe("load.js", "// load, edited\n");
    let changed = world.run(
        &fake,
        &["run", "--before", "session", "--dry-run", "--json"],
    );
    assert_eq!(changed.exit, Exit::Refused, "{}", changed.err);
    assert_eq!(changed.line()["failure"]["phase"], "seal");
    let live = world.run(&fake, &["run", "--dry-run"]);
    assert_eq!(live.exit, Exit::Passed, "{}", live.err);
    assert!(
        live.out.contains("compose.yaml (live folder)")
            && live.out.ends_with("\nsetup head built\n"),
        "another recipe, another key: {}",
        live.out
    );
    let small = |dir: &Utf8Path| {
        let mut host = roomy(dir)?;
        host.cpus = 2;
        Ok(host)
    };
    let ran = world.run_on(
        &fake,
        &["run", "--dry-run", "--json"],
        &small,
        &Cancel::new(),
    );
    assert_eq!(ran.exit, Exit::Refused, "{}", ran.err);
    assert_eq!(ran.line()["failure"]["phase"], "budget");
    assert!(!fake.journal()[mark..].iter().any(made));
}
