//! `snapshot`, `guide`, `init` and `--help`, which open no stand folder.

use std::fs;

use camino::Utf8Path;
use cli::Exit;
use run::Cancel;

use super::{World, any_create, fake, roomy};

#[test]
fn snapshot_prints_its_sha_and_then_the_one_it_replaced() {
    let world = World::new();
    let fake = fake(Vec::new());
    let first = world.run(&fake, &["snapshot"]);
    assert_eq!(first.exit, Exit::Passed, "{}", first.err);
    let sha = first.out.trim().to_owned();
    assert_eq!(sha.len(), 40, "{sha}");
    let index = world.repo.git(&["status", "--porcelain"]);
    world.repo.write("main.js", "console.log('v2')\n");
    let second = world.run(&fake, &["snapshot"]);
    assert!(
        second.out.ends_with(&format!("\nreplaced {sha}\n")),
        "{}",
        second.out
    );
    world.repo.write("main.js", "console.log('v1')\n");
    assert_eq!(
        world.repo.git(&["status", "--porcelain"]),
        index,
        "the index stays"
    );
    assert!(!any_create(&fake));
}

#[test]
fn guide_prints_the_contents_and_each_topic_and_refuses_another() {
    let world = World::new();
    let fake = fake(Vec::new());
    // The guide needs no repository: an agent reads it before it has a stand file.
    let outside = tempfile::tempdir().unwrap();
    let outside = Utf8Path::from_path(outside.path()).unwrap();
    let contents = world.run_at(outside, &fake, &["guide"], &roomy, &Cancel::new());
    assert_eq!(contents.exit, Exit::Passed, "{}", contents.err);
    assert!(
        contents.out.contains("The shortest path"),
        "{}",
        contents.out
    );
    assert!(contents.out.contains("- `130`: "), "{}", contents.out);
    assert!(!contents.out.contains("## "), "{}", contents.out);
    let last = contents.out.lines().last();
    let topics = "Topics: recipe, probes, expect, line, inspect. `stop-slop-stand guide <topic>` prints one.";
    assert_eq!(last, Some(topics));
    for topic in ["recipe", "expect", "line", "inspect"] {
        let ran = world.run(&fake, &["guide", topic]);
        assert_eq!(ran.exit, Exit::Passed, "{topic}");
        let heading = format!("## {topic}");
        assert_eq!(ran.out.lines().next(), Some(heading.as_str()));
        let other = ran.out.lines().skip(1).find(|line| line.starts_with("## "));
        assert_eq!(other, None, "{topic} prints only its section");
    }
    let probes = world.run(&fake, &["guide", "probes"]);
    assert_eq!(probes.out.lines().next(), Some("# probes"));
    assert!(probes.out.contains("$$"));
    let unknown = world.run(&fake, &["guide", "nothing"]);
    assert_eq!(unknown.exit, Exit::Refused);
    assert!(
        unknown
            .err
            .contains("topics: recipe, probes, expect, line, inspect"),
        "{}",
        unknown.err
    );
    assert_eq!(unknown.next(), ["stop-slop-stand guide"]);
    assert!(fake.journal().is_empty());
}

#[test]
fn init_writes_a_template_that_loads_and_refuses_a_second_time() {
    let world = World::with("");
    let fake = fake(Vec::new());
    let ran = world.run(&fake, &["init"]);
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    let file = world.recipe().join("compose.yaml");
    // The path from git, which resolves the temp folder: `/private/var` for `/var` on macOS, the
    // long name for `RUNNER~1` on Windows.
    let wrote = ran.out.trim_end().strip_prefix("wrote ").expect(&ran.out);
    assert!(!wrote.contains('\\'), "{wrote}");
    assert_eq!(
        Utf8Path::new(wrote).canonicalize_utf8().unwrap(),
        file.canonicalize_utf8().unwrap()
    );
    assert!(spec::load(&file, &spec::Policy::new(None).unwrap()).is_ok());
    let dry = world.run(&fake, &["run", "--dry-run"]);
    assert_eq!(dry.exit, Exit::Passed, "{}", dry.err);
    let mine = format!("{}# mine\n", fs::read_to_string(&file).unwrap());
    fs::write(&file, &mine).unwrap();
    let again = world.run(&fake, &["init"]);
    assert_eq!(again.exit, Exit::Refused);
    assert!(again.err.contains("is there already"), "{}", again.err);
    assert_eq!(again.next(), ["stop-slop-stand guide recipe"]);
    assert_eq!(fs::read_to_string(&file).unwrap(), mine);
    assert!(!any_create(&fake));
    let outside = tempfile::tempdir().unwrap();
    let outside = Utf8Path::from_path(outside.path()).unwrap();
    let refused = world.run_at(outside, &fake, &["init"], &roomy, &Cancel::new());
    assert_eq!(refused.exit, Exit::Refused, "{}", refused.err);
    assert!(
        refused.err.contains("not inside a git work tree"),
        "{}",
        refused.err
    );
}

#[test]
fn help_holds_at_most_twenty_lines_and_ends_with_the_guide() {
    let world = World::new();
    let ran = world.run(&fake(Vec::new()), &["--help"]);
    assert_eq!(ran.exit, Exit::Passed);
    assert!(ran.out.lines().count() <= 20, "{}", ran.out);
    for command in [
        "run", "snapshot", "inspect", "accept", "guide", "init", "clean",
    ] {
        let listed = ran
            .out
            .lines()
            .any(|line| line.trim_start().starts_with(command));
        assert!(listed, "{command}: {}", ran.out);
    }
    assert_eq!(
        ran.out.trim_end().lines().last(),
        Some("agents: run `guide`")
    );
}

/// `main` runs `guide`, `init` and `snapshot` before it opens the stand folder or starts the
/// adapter (batch review 6): they work where the folder cannot be made, and `guide`, the `next`
/// of that failure, works after it.
#[test]
fn the_local_commands_open_no_stand_folder() {
    let world = World::with("");
    let base = Utf8Path::from_path(world.dir.path()).unwrap().to_owned();
    // A file where the stand folder's parent would be: no folder can be made under it.
    fs::write(base.join("file"), "").unwrap();
    let home = base.join("file").join("home");
    let stand = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_stop-slop-stand"))
            .args(args)
            .current_dir(&world.cwd)
            .env("STOP_SLOP_STAND_HOME", &home)
            .output()
            .unwrap()
    };
    let run = stand(&["run", "--json"]);
    assert_eq!(run.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.ends_with("next: stop-slop-stand guide\n"),
        "{stderr}"
    );
    for args in [
        &["--help"][..],
        &["guide"],
        &["guide", "recipe"],
        &["init"],
        &["snapshot"],
    ] {
        let local = stand(args);
        let stderr = String::from_utf8_lossy(&local.stderr);
        assert!(local.status.success(), "{args:?}: {stderr}");
    }
}
