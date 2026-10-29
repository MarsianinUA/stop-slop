//! The sweep of the start, the TTL, a low disk and `clean`.

use std::cell::Cell;
use std::fs;
use std::time::{Duration, SystemTime};

use camino::Utf8Path;
use cli::Exit;
use run::Cancel;
use sandbox::{Call, Fake, ImageRef, Launch, Sandbox};
use serde_json::Value;

use super::{
    GIB, World, before_first_create, cached_of, dropped, fake, folders, kept_of, read_json, roomy,
    set, snapshots_left,
};

#[test]
fn the_start_sweeps_leftovers_but_keeps_kept_and_cached_snapshots_and_the_ttl_forgets() {
    let world = World::new();
    let fake = fake(Vec::new());
    assert_eq!(world.run(&fake, &["run"]).exit, Exit::Passed);
    let session = world.sessions().remove(0);
    let (kept, cached) = (kept_of(&session), cached_of(&session));
    assert!(!kept.is_empty() && !cached.is_empty());
    assert_eq!(set(&snapshots_left(&fake)), &set(&kept) | &set(&cached));
    // A crashed run's VM and snapshot, which no run or slot names, and its tar.
    let stray = fake.create(&[db_launch(&fake)]).unwrap();
    let stray_snapshot = fake.snapshot(&stray[0]).unwrap();
    fs::create_dir_all(world.paths.root.join("trees")).unwrap();
    fs::write(world.tar("head"), b"partial").unwrap();
    // A dry run counts them and drops nothing (orchestrator, review of T-14).
    let mark = fake.journal().len();
    let dry = world.run(&fake, &["run", "--dry-run"]);
    let said = "\nwould destroy 1 VM that earlier runs left\n\
                would drop 1 snapshot that no kept run or cache entry names\n";
    assert!(dry.out.contains(said), "{}", dry.out);
    let swept = |call: &Call| matches!(call, Call::Destroy(_) | Call::DropSnapshot(_));
    assert!(!fake.journal()[mark..].iter().any(swept));
    let mark = fake.journal().len();
    assert_eq!(world.run(&fake, &["run"]).exit, Exit::Passed);
    let swept = before_first_create(&fake, mark);
    assert!(
        swept
            .iter()
            .any(|call| matches!(call, Call::Destroy(id) if *id == stray[0]))
    );
    assert_eq!(
        dropped(&swept),
        [stray_snapshot],
        "only the stray goes at the start"
    );
    // The second run is kept now, so the first's snapshots go; the cache entry stays in its slot.
    let left = set(&snapshots_left(&fake));
    assert!(
        set(&kept).is_disjoint(&left),
        "the first kept run's snapshots go"
    );
    assert!(set(&cached).is_subset(&left), "the cache entry stays");
    // The TTL: a session not used for three days is forgotten and its snapshots go; a run of
    // another session of the same stand folder sweeps it.
    age_sessions(&world, Duration::from_secs(4 * 24 * 3600));
    let other = world.worktree("other");
    let dry = world.run_at(&other, &fake, &["run", "--dry-run"], &roomy, &Cancel::new());
    let said = "\nwould forget the runs and cache of 1 expired session\n";
    assert!(dry.out.contains(said), "{}", dry.out);
    assert!(session.exists(), "the dry run forgets nothing");
    let ran = world.run_at(&other, &fake, &["run"], &roomy, &Cancel::new());
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    let now = set(&snapshots_left(&fake));
    assert!(left.is_disjoint(&now), "the expired session's snapshots go");
    assert!(!session.exists(), "its folder goes");
    assert_eq!(world.sessions().len(), 1);
    let prune = fake
        .journal()
        .into_iter()
        .rev()
        .find_map(|call| match call {
            Call::PruneImages(keep) => Some(keep),
            _ => None,
        });
    assert!(
        prune.is_some_and(|keep| keep.contains(&ImageRef::new("postgres:17"))),
        "the live session keeps its images"
    );
}

/// A `Launch` of `db` as the first run made it, for a VM a crashed run would leave.
fn db_launch(fake: &Fake) -> Launch {
    fake.journal()
        .into_iter()
        .find_map(|call| match call {
            Call::Create { batch, .. } => batch
                .into_iter()
                .find(|launch| launch.service == "db" && launch.seed.is_none()),
            _ => None,
        })
        .expect("the first run created db")
}

/// `usedAt` of every session of the world, moved back (store's `session.json`, a law of the blueprint).
fn age_sessions(world: &World, by: Duration) {
    for session in world.sessions() {
        let path = session.join("session.json");
        let mut state = read_json(&path);
        let old = SystemTime::now() - by;
        state["usedAt"] = Value::String(humantime::format_rfc3339(old).to_string());
        fs::write(&path, state.to_string()).unwrap();
    }
}

#[test]
fn a_low_disk_drops_the_cache_and_kept_run_of_others_then_its_own_cache_then_its_kept_run() {
    let world = World::new();
    let fake = fake(Vec::new());
    let other = world.worktree("other");
    let ran = world.run_at(&other, &fake, &["run"], &roomy, &Cancel::new());
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    let others = set(&snapshots_left(&fake));
    let theirs = world.sessions();
    assert_eq!(world.run(&fake, &["run"]).exit, Exit::Passed);
    let own = world
        .sessions()
        .into_iter()
        .find(|session| !theirs.contains(session))
        .unwrap();
    let (own_kept, own_cached) = (set(&kept_of(&own)), set(&cached_of(&own)));
    assert!(!own_kept.is_empty() && !own_cached.is_empty());
    // A dry run on a low disk drops nothing and says what a run would drop, in turn; the kept run
    // still serves `inspect` after it (orchestrator, review of T-14).
    let low = |dir: &Utf8Path| {
        let mut host = roomy(dir)?;
        host.free_disk = 10 * GIB;
        Ok(host)
    };
    let kept_run = |session: &Utf8Path| {
        let state = read_json(&session.join("session.json"));
        state["kept"].as_str().unwrap().to_owned()
    };
    let (own_run, their_run) = (kept_run(&own), kept_run(&theirs[0]));
    let dry_mark = fake.journal().len();
    let dry = world.run_on(&fake, &["run", "--dry-run"], &low, &Cancel::new());
    assert_eq!(dry.exit, Exit::Passed, "{}", dry.err);
    let said = [
        "disk: 10.0 GiB free on the volume of the stand's folder, under the 20.0 GiB it keeps \
         free for snapshots: a run drops in turn until there is room"
            .to_owned(),
        format!("would drop 1 cache entry and 1 kept run ({their_run}) of other sessions"),
        "would drop 1 cache entry of this session".to_owned(),
        format!("would drop 1 kept run ({own_run}) of this session"),
    ];
    let lines: Vec<&str> = dry.out.lines().skip(1).take(4).collect();
    assert_eq!(lines, said, "{}", dry.out);
    let touched = |call: &Call| {
        matches!(
            call,
            Call::DropSnapshot(_) | Call::Destroy(_) | Call::Create { .. } | Call::Pull(_)
        )
    };
    assert!(!fake.journal()[dry_mark..].iter().any(touched));
    assert_eq!(set(&kept_of(&own)), own_kept);
    assert_eq!(set(&cached_of(&own)), own_cached);
    assert!(own_kept.is_subset(&set(&snapshots_left(&fake))));
    let looked = world.run(&fake, &["inspect", &own_run, "head", "db", "--", "true"]);
    assert_eq!(looked.exit, Exit::Passed, "{}", looked.err);
    // A run the CPUs refuse keeps all of it on that disk: only the disk is worth what it keeps.
    let small = |dir: &Utf8Path| {
        let mut host = low(dir)?;
        host.cpus = 2;
        Ok(host)
    };
    let mark = fake.journal().len();
    let ran = world.run_on(&fake, &["run", "--json"], &small, &Cancel::new());
    assert_eq!(ran.exit, Exit::Refused, "{}", ran.err);
    let cause = ran.line()["failure"]["cause"].to_string();
    assert!(cause.contains("cpu: requested"), "{cause}");
    assert!(dropped(&fake.journal()[mark..]).is_empty());
    // Low until the third read: the other session's snapshots and this one's cache go, its kept
    // run stays.
    let reads = Cell::new(0);
    let host = |dir: &Utf8Path| {
        reads.set(reads.get() + 1);
        let mut host = roomy(dir)?;
        if reads.get() < 3 {
            host.free_disk = 10 * GIB;
        }
        Ok(host)
    };
    let mark = fake.journal().len();
    let ran = world.run_on(&fake, &["run"], &host, &Cancel::new());
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    assert_eq!(reads.get(), 3);
    let dropped = dropped(&before_first_create(&fake, mark));
    let (first, then) = dropped.split_at(others.len().min(dropped.len()));
    assert_eq!(set(first), others, "the others' go first");
    assert_eq!(
        set(then),
        own_cached,
        "then this session's cache, and not its kept run"
    );
    // Its setup came from no cache: the slot was gone.
    let own_runs = folders(&own.join("runs"));
    let last = own_runs.last().unwrap();
    let report = read_json(&last.join("report.json"));
    assert!(report["head"]["setup"]["cached"].is_null());
    // The disk low to the end: the kept run goes too, and the budget refuses.
    let low = |dir: &Utf8Path| {
        let mut host = roomy(dir)?;
        host.free_disk = 10 * GIB;
        Ok(host)
    };
    let kept = set(&kept_of(&own));
    let ran = world.run_on(&fake, &["run", "--json"], &low, &Cancel::new());
    assert_eq!(ran.exit, Exit::Refused, "{}", ran.err);
    assert_eq!(ran.line()["failure"]["phase"], "budget");
    assert!(
        kept.is_disjoint(&set(&snapshots_left(&fake))),
        "its kept run went too"
    );
}

#[test]
fn clean_drops_snapshots_before_images_and_keeps_the_journal() {
    let world = World::new();
    let fake = fake(Vec::new());
    world.run(&fake, &["snapshot"]);
    world.repo.write("main.js", "console.log('v2')\n");
    let sealed = world.run(&fake, &["run", "--before", "session"]);
    assert_eq!(sealed.exit, Exit::Passed, "{}", sealed.err);
    assert!(!snapshots_left(&fake).is_empty());
    let ran = world.run(&fake, &["clean"]);
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    let journal = fake.journal();
    let last_drop = journal
        .iter()
        .rposition(|call| matches!(call, Call::DropSnapshot(_)))
        .unwrap();
    let prune = journal
        .iter()
        .rposition(|call| matches!(call, Call::PruneImages(keep) if keep.is_empty()))
        .unwrap();
    assert!(last_drop < prune);
    assert!(snapshots_left(&fake).is_empty());
    assert!(!world.paths.root.join("sessions").exists());
    let journals: Vec<String> = fs::read_dir(world.paths.root.join("journal"))
        .unwrap()
        .flatten()
        .map(|entry| fs::read_to_string(entry.path()).unwrap())
        .collect();
    assert!(
        journals.len() == 1 && journals[0].contains("cleanedAt"),
        "{journals:?}"
    );
    assert_eq!(
        world.repo.git(&["for-each-ref", "refs/stop-slop/stand"]),
        "",
        "the snapshot ref goes"
    );
    // The next seal shows the clean in its journal.
    world.run(&fake, &["snapshot"]);
    world.repo.write("main.js", "console.log('v3')\n");
    let line = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    let verbs: Vec<&str> = line["seal"]["journal"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            let mut keys = entry.as_object().unwrap().keys();
            keys.find(|key| key.ends_with("At")).unwrap().as_str()
        })
        .collect();
    assert_eq!(verbs, ["sealedAt", "cleanedAt", "sealedAt"], "{line}");
    // `--all` leaves the lock and nothing else of the folder (`stand.toml` lies outside it here).
    let mark = fake.journal().len();
    assert_eq!(world.run(&fake, &["clean", "--all"]).exit, Exit::Passed);
    assert!(fake.journal().len() == mark, "no call of the adapter");
    let left: Vec<String> = fs::read_dir(&world.paths.root)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["lock"]);
    assert_eq!(
        world.repo.git(&["for-each-ref", "refs/stop-slop/stand"]),
        ""
    );
}
