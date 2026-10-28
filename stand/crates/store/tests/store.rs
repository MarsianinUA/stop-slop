use std::collections::BTreeSet;
use std::fs;
use std::time::{Duration, SystemTime};

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{Value, json};
use store::{
    Acceptance, CacheEntry, Clean, ConfigError, JournalEntry, KeptScope, PoolKey, RunPaths,
    Session, SetupKey, Slot, Store, StorePaths,
};
use tempfile::TempDir;

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// A store in a temporary folder, with stand folders and worktrees beside it.
struct Home {
    _temp: TempDir,
    dir: Utf8PathBuf,
    store: Store,
}

fn home() -> Home {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8Path::from_path(temp.path()).unwrap().to_owned();
    let store = Store::open(StorePaths {
        root: dir.join("home"),
        config: dir.join("config").join("stand.toml"),
    })
    .unwrap();
    Home {
        _temp: temp,
        dir,
        store,
    }
}

impl Home {
    fn root(&self) -> Utf8PathBuf {
        self.dir.join("home")
    }

    /// `<dir>/<worktree>/stand/compose.yaml` with a harness file and an empty folder.
    fn stand(&self, worktree: &str) -> Utf8PathBuf {
        let stand = self.dir.join(worktree).join("stand");
        fs::create_dir_all(stand.join("scripts")).unwrap();
        fs::create_dir_all(stand.join("empty")).unwrap();
        fs::write(stand.join("compose.yaml"), "services: {}\n").unwrap();
        fs::write(stand.join("scripts").join("load.js"), "load()\n").unwrap();
        stand
    }

    fn session(&self, worktree: &str, images: &[&str], now: SystemTime) -> Session {
        let stand = self.stand(worktree);
        let images: Vec<String> = images.iter().map(|image| image.to_string()).collect();
        self.store
            .session(
                &stand.join("compose.yaml"),
                &self.dir.join(worktree),
                &images,
                now,
            )
            .unwrap()
    }
}

/// 2026-10-28T12:00:00Z and `seconds` after.
fn at(seconds: u64) -> SystemTime {
    humantime::parse_rfc3339("2026-10-28T12:00:00Z").unwrap() + Duration::from_secs(seconds)
}

fn pool_key(head: &str) -> PoolKey {
    PoolKey {
        head_tree: head.to_owned(),
        before_tree: Some("b".repeat(40)),
        digest: "d".repeat(64),
    }
}

/// Two images; `tree` tells keys apart.
fn setup_key(tree: &str) -> SetupKey {
    SetupKey {
        tree: tree.to_owned(),
        recipe: "r".repeat(64),
        images: vec![
            ("node:22".to_owned(), "sha256:1".to_owned()),
            ("postgres:17".to_owned(), "sha256:2".to_owned()),
        ],
        guest: "0.1.0".to_owned(),
        tool: "0.1.0".to_owned(),
    }
}

fn entry(dir: &Utf8Path, built_by: &str) -> CacheEntry {
    CacheEntry {
        dir: dir.to_owned(),
        built_by: built_by.to_owned(),
    }
}

fn ids(runs: &[RunPaths]) -> Vec<&str> {
    runs.iter().map(|run| run.id.as_str()).collect()
}

fn json_of(path: &Utf8Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn names_in(dir: &Utf8Path) -> BTreeSet<String> {
    dir.read_dir_utf8()
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_owned())
        .collect()
}

#[test]
fn a_second_lock_is_refused_until_the_first_is_dropped() {
    let home = home();
    let lock = home.store.try_lock().unwrap().expect("a free lock");
    assert!(home.store.try_lock().unwrap().is_none());
    let other = Store::open(StorePaths {
        root: home.root(),
        config: home.dir.join("stand.toml"),
    })
    .unwrap();
    assert!(other.try_lock().unwrap().is_none());
    drop(lock);
    assert!(other.try_lock().unwrap().is_some());
}

#[test]
fn config_reads_both_keys_and_names_the_key_of_each_error() {
    let home = home();
    let config = home.store.config().unwrap();
    assert_eq!((config.host_share, config.max_tolerance), (None, None));

    let file = home.dir.join("config").join("stand.toml");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "host_share = 60\nmax_tolerance = 40\n").unwrap();
    let config = home.store.config().unwrap();
    assert_eq!(
        (config.host_share, config.max_tolerance),
        (Some(60), Some(40))
    );

    for (text, key, words) in [
        ("hostshare = 50\n", "hostshare", "unknown key"),
        ("host_share = \"50\"\n", "host_share", "found \"50\""),
        (
            "host_share = 300\n",
            "host_share",
            "from 0 to 255, found 300",
        ),
        ("max_tolerance = 2.5\n", "max_tolerance", "found 2.5"),
        ("host_share = 50\nmax_tolerance =\n", "toml", "line 2"),
        ("[host_share\n", "toml", "line 1"),
    ] {
        fs::write(&file, text).unwrap();
        let ConfigError {
            file: refused,
            key: refused_key,
            message,
        } = home.store.config().expect_err(text);
        assert_eq!(refused, file, "{text}");
        assert_eq!(refused_key, key, "{text}: {message}");
        assert!(message.contains(words), "{text}: {message}");
    }
}

#[test]
fn a_session_is_one_stand_file_in_one_worktree() {
    let home = home();
    let first = home.session("one", &[], at(0));
    let other_worktree = home.session("two", &[], at(0));
    fs::write(home.dir.join("one").join("other.yaml"), "").unwrap();
    let other_file = home
        .store
        .session(
            &home.dir.join("one").join("other.yaml"),
            &home.dir.join("one"),
            &[],
            at(0),
        )
        .unwrap();
    fs::create_dir_all(home.dir.join("one").join("sub")).unwrap();
    let same = home
        .store
        .session(
            &home.dir.join("one/sub/../stand/compose.yaml"),
            &home.dir.join("one/sub/.."),
            &[],
            at(1),
        )
        .unwrap();
    assert_ne!(first.key(), other_worktree.key());
    assert_ne!(first.key(), other_file.key());
    assert_eq!(first.key(), same.key());
}

#[test]
fn runs_pool_by_key_and_the_previous_report_is_the_last_passed_run_of_another_key() {
    let home = home();
    let session = home.session("work", &[], at(0));
    let (one, two) = (pool_key("1"), pool_key("2"));
    let first = session.new_run(at(0), &one).unwrap();
    session.passed(&first).unwrap();
    let failed = session.new_run(at(0), &one).unwrap();
    let second = session.new_run(at(10), &two).unwrap();
    session.passed(&second).unwrap();
    let third = session.new_run(at(20), &two).unwrap();
    session.passed(&third).unwrap();

    assert_eq!(first.id, "20261028-120000");
    assert_eq!(
        failed.id, "20261028-120001",
        "the same second takes the next"
    );
    assert!(failed.dir.is_dir() && failed.dir != first.dir);
    assert_eq!(ids(&session.pool(&one).unwrap()), [first.id.as_str()]);
    assert_eq!(
        ids(&session.pool(&two).unwrap()),
        [second.id.as_str(), third.id.as_str()]
    );
    assert_eq!(session.previous_report(&two).unwrap().unwrap().id, first.id);
    assert_eq!(session.previous_report(&one).unwrap().unwrap().id, third.id);
    assert!(session.previous_report(&pool_key("3")).unwrap().is_some());

    let history = session.history().unwrap();
    let seen: Vec<(&str, SystemTime, &str, bool)> = history
        .iter()
        .map(|run| {
            (
                run.id.as_str(),
                run.created_at,
                run.key.head_tree.as_str(),
                run.passed,
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            (first.id.as_str(), at(0), "1", true),
            (failed.id.as_str(), at(0), "1", false),
            (second.id.as_str(), at(10), "2", true),
            (third.id.as_str(), at(20), "2", true),
        ]
    );
    assert_eq!(
        json_of(&first.dir.join("run.json")),
        json!({
            "key": {"headTree": "1", "beforeTree": "b".repeat(40), "digest": "d".repeat(64)},
            "createdAt": "2026-10-28T12:00:00Z",
            "passed": true,
        })
    );
}

#[test]
fn a_seal_copies_the_folder_and_only_a_reseal_changes_the_copy() {
    let home = home();
    let session = home.session("work", &[], at(0));
    let stand = home.dir.join("work").join("stand");
    assert!(session.sealed().unwrap().is_none());

    let sealed = session.seal(&stand, "digest-1", at(1)).unwrap();
    assert_eq!(sealed.sealed_at, at(1));
    let copy = sealed.dir.clone();
    assert_eq!(names_in(&copy), names_in(&stand));
    assert!(copy.join("empty").is_dir());
    assert_eq!(
        fs::read_to_string(copy.join("scripts").join("load.js")).unwrap(),
        "load()\n"
    );

    fs::write(stand.join("compose.yaml"), "services: { api: {} }\n").unwrap();
    fs::write(stand.join("new.js"), "new()\n").unwrap();
    fs::remove_file(stand.join("scripts").join("load.js")).unwrap();
    let still = session.sealed().unwrap().unwrap();
    assert_eq!(
        (still.dir.as_path(), still.sealed_at),
        (copy.as_path(), at(1))
    );
    assert_eq!(
        fs::read_to_string(copy.join("compose.yaml")).unwrap(),
        "services: {}\n"
    );
    assert!(!copy.join("new.js").exists());
    assert!(copy.join("scripts").join("load.js").is_file());

    let resealed = session
        .reseal(&stand, "digest-2", at(2), "the probe reads one more table")
        .unwrap();
    assert_eq!(
        (resealed.dir.as_path(), resealed.sealed_at),
        (copy.as_path(), at(2))
    );
    assert_eq!(
        fs::read_to_string(copy.join("compose.yaml")).unwrap(),
        "services: { api: {} }\n"
    );
    assert!(copy.join("new.js").is_file());
    assert!(!copy.join("scripts").join("load.js").exists());
    assert!(matches!(
        &resealed.journal[..],
        [
            JournalEntry::Sealed { digest: first, .. },
            JournalEntry::Resealed { digest: second, reason, .. },
        ] if first == "digest-1" && second == "digest-2" && reason == "the probe reads one more table"
    ));
    let journal = home
        .root()
        .join("journal")
        .join(format!("{}.json", session.key()));
    assert_eq!(
        json_of(&journal),
        json!([
            {"sealedAt": "2026-10-28T12:00:01Z", "digest": "digest-1"},
            {
                "resealedAt": "2026-10-28T12:00:02Z",
                "digest": "digest-2",
                "reason": "the probe reads one more table",
            },
        ])
    );
}

#[cfg(unix)]
#[test]
fn a_seal_keeps_a_link_a_link() {
    let home = home();
    let session = home.session("work", &[], at(0));
    let stand = home.dir.join("work").join("stand");
    std::os::unix::fs::symlink("scripts/load.js", stand.join("load.js")).unwrap();
    let sealed = session.seal(&stand, "digest", at(1)).unwrap();
    assert_eq!(
        fs::read_link(sealed.dir.join("load.js")).unwrap(),
        Utf8Path::new("scripts/load.js")
    );
}

#[test]
fn acceptances_add_up_and_outlive_a_run() {
    let home = home();
    let session = home.session("work", &[], at(0));
    let accept = |measure: &str, allow: &str, seconds: u64| Acceptance {
        measure: measure.to_owned(),
        allow: allow.to_owned(),
        reason: format!("{allow} on purpose"),
        head_tree: "h".repeat(40),
        accepted_at: at(seconds),
    };
    session.accept(accept("stand.wall", "+10%", 1)).unwrap();
    session.accept(accept("stand.wall", "+15%", 2)).unwrap();
    session.accept(accept("db.memory", "down", 3)).unwrap();
    session.new_run(at(4), &pool_key("1")).unwrap();
    let seen: Vec<(String, String)> = session
        .acceptances()
        .unwrap()
        .into_iter()
        .map(|acceptance| (acceptance.measure, acceptance.allow))
        .collect();
    assert_eq!(
        seen,
        [
            ("stand.wall".to_owned(), "+10%".to_owned()),
            ("stand.wall".to_owned(), "+15%".to_owned()),
            ("db.memory".to_owned(), "down".to_owned()),
        ]
    );
    let file = home
        .root()
        .join("sessions")
        .join(session.key())
        .join("accepted.json");
    assert_eq!(
        json_of(&file)[0],
        json!({
            "measure": "stand.wall",
            "allow": "+10%",
            "reason": "+10% on purpose",
            "headTree": "h".repeat(40),
            "acceptedAt": "2026-10-28T12:00:01Z",
        })
    );
}

#[test]
fn the_kept_run_is_the_last_one_kept_and_drop_kept_forgets_by_scope() {
    let home = home();
    let mine = home.session("mine", &[], at(0));
    let theirs = home.session("theirs", &[], at(0));
    let first = mine.new_run(at(1), &pool_key("1")).unwrap();
    let second = mine.new_run(at(2), &pool_key("1")).unwrap();
    let other = theirs.new_run(at(3), &pool_key("1")).unwrap();
    assert!(mine.kept().unwrap().is_none());
    assert!(mine.keep(&first).unwrap().is_none());
    assert_eq!(mine.keep(&second).unwrap().unwrap().id, first.id);
    assert_eq!(mine.kept().unwrap().unwrap().id, second.id);
    theirs.keep(&other).unwrap();

    let kept: BTreeSet<String> = home
        .store
        .kept_runs()
        .unwrap()
        .into_iter()
        .map(|run| run.id)
        .collect();
    assert_eq!(kept, BTreeSet::from([second.id.clone(), other.id.clone()]));

    let dropped = home
        .store
        .drop_kept(KeptScope::OtherThan(mine.key().to_owned()))
        .unwrap();
    assert_eq!(ids(&dropped), [other.id.as_str()]);
    assert_eq!(dropped[0].dir, other.dir);
    assert_eq!(mine.kept().unwrap().unwrap().id, second.id);
    assert!(theirs.kept().unwrap().is_none());
    assert_eq!(
        ids(&home.store.drop_kept(KeptScope::All).unwrap()),
        [second.id.as_str()]
    );
    assert!(home.store.kept_runs().unwrap().is_empty());
}

#[test]
fn the_ttl_forgets_a_session_and_what_it_keeps_but_not_its_journal() {
    let home = home();
    let old = home.session("old", &["postgres:17", "node:22"], at(0));
    let live = home.session("live", &["redis:7", "node:22"], at(0) + 2 * DAY);
    let run = old.new_run(at(1), &pool_key("1")).unwrap();
    old.passed(&run).unwrap();
    old.keep(&run).unwrap();
    old.seal(&home.dir.join("old").join("stand"), "digest", at(2))
        .unwrap();
    old.accept(Acceptance {
        measure: "stand.wall".to_owned(),
        allow: "+10%".to_owned(),
        reason: "one more lookup".to_owned(),
        head_tree: "h".repeat(40),
        accepted_at: at(3),
    })
    .unwrap();
    let old_key = setup_key("old");
    let old_entry = old.new_entry(&old_key).unwrap();
    old.cache(Slot::Before, &old_key, entry(&old_entry, "run-1"))
        .unwrap();
    let live_key = SetupKey {
        images: vec![("mysql:8".to_owned(), "sha256:3".to_owned())],
        ..setup_key("live")
    };
    let live_entry = live.new_entry(&live_key).unwrap();
    live.cache(Slot::Head, &live_key, entry(&live_entry, "run-2"))
        .unwrap();

    assert!(
        home.store
            .expired(at(0) + 3 * DAY - Duration::from_secs(1))
            .unwrap()
            .is_empty()
    );
    let expired = home.store.expired(at(0) + 3 * DAY).unwrap();
    assert_eq!(expired.len(), 1);
    home.store.forget(&expired).unwrap();

    assert!(old.history().unwrap().is_empty());
    assert!(old.sealed().unwrap().is_none());
    assert!(old.acceptances().unwrap().is_empty());
    assert!(!run.dir.exists());
    assert!(!old_entry.exists());
    assert!(live_entry.is_dir());
    assert_eq!(ids(&home.store.kept_runs().unwrap()), Vec::<&str>::new());
    assert_eq!(
        home.store.kept_images().unwrap(),
        ["mysql:8", "node:22", "redis:7"]
    );
    let journal = home
        .root()
        .join("journal")
        .join(format!("{}.json", old.key()));
    assert_eq!(json_of(&journal).as_array().unwrap().len(), 1);
    assert!(live.history().unwrap().is_empty());
    assert_eq!(home.store.expired(at(0) + 3 * DAY).unwrap().len(), 0);
}

#[test]
fn clean_sessions_keeps_the_install_and_the_journals_and_clean_all_keeps_the_lock() {
    let home = home();
    let root = home.root();
    fs::create_dir_all(root.join("msb").join("bin")).unwrap();
    fs::create_dir_all(root.join("guest")).unwrap();
    let lock = home.store.try_lock().unwrap().unwrap();
    let one = home.session("one", &[], at(0));
    let two = home.session("two", &[], at(0));
    one.seal(&home.dir.join("one").join("stand"), "digest-1", at(1))
        .unwrap();
    two.seal(&home.dir.join("two").join("stand"), "digest-2", at(1))
        .unwrap();
    let key = setup_key("tree");
    let cached = one.new_entry(&key).unwrap();
    one.cache(Slot::Head, &key, entry(&cached, "run-1"))
        .unwrap();
    let trees = home.store.trees().unwrap();
    fs::write(&trees.head, "tar").unwrap();

    home.store.clean(Clean::Sessions).unwrap();
    assert_eq!(
        names_in(&root),
        BTreeSet::from(["guest", "journal", "lock", "msb"].map(str::to_owned))
    );
    assert!(!cached.exists());
    for session in [&one, &two] {
        assert!(session.sealed().unwrap().is_none());
        let journal = root.join("journal").join(format!("{}.json", session.key()));
        let entries = json_of(&journal);
        assert_eq!(entries.as_array().unwrap().len(), 2);
        assert!(entries[1]["cleanedAt"].is_string(), "{entries}");
    }
    let again = home.session("one", &[], at(5));
    let sealed = again
        .seal(&home.dir.join("one").join("stand"), "digest-3", at(6))
        .unwrap();
    assert!(matches!(
        &sealed.journal[..],
        [
            JournalEntry::Sealed { .. },
            JournalEntry::Cleaned { .. },
            JournalEntry::Sealed { .. }
        ]
    ));

    home.store.clean(Clean::All).unwrap();
    assert_eq!(names_in(&root), BTreeSet::from(["lock".to_owned()]));
    drop(lock);
    assert!(home.store.try_lock().unwrap().is_some());
}

#[test]
fn clean_all_keeps_a_config_in_the_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = Utf8Path::from_path(temp.path())
        .unwrap()
        .join("stop-slop-stand");
    let store = Store::open(StorePaths {
        root: root.clone(),
        config: root.join("stand.toml"),
    })
    .unwrap();
    fs::write(root.join("stand.toml"), "host_share = 60\n").unwrap();
    store.trees().unwrap();
    store.clean(Clean::All).unwrap();
    assert_eq!(names_in(&root), BTreeSet::from(["stand.toml".to_owned()]));
    assert_eq!(store.config().unwrap().host_share, Some(60));
}

#[test]
fn the_trees_are_two_tar_files_that_clear_trees_deletes() {
    let home = home();
    let trees = home.store.trees().unwrap();
    assert_eq!(trees.head, home.root().join("trees").join("head.tar"));
    assert_eq!(trees.before, home.root().join("trees").join("before.tar"));
    fs::write(&trees.head, "head").unwrap();
    fs::write(&trees.before, "before").unwrap();
    home.store.clear_trees().unwrap();
    assert!(!trees.head.exists() && !trees.before.exists());
    home.store.clear_trees().unwrap();
}

#[test]
fn a_setup_key_digest_changes_with_every_part() {
    let key = setup_key("tree");
    assert_eq!(key.digest(), setup_key("tree").digest());
    assert_eq!(key.digest().len(), 64);
    let mut other_image = key.clone();
    other_image.images[1].1 = "sha256:9".to_owned();
    let mut one_image_less = key.clone();
    one_image_less.images.pop();
    for other in [
        SetupKey {
            tree: "other".to_owned(),
            ..key.clone()
        },
        SetupKey {
            recipe: "s".repeat(64),
            ..key.clone()
        },
        other_image,
        one_image_less,
        SetupKey {
            guest: "dev-0123".to_owned(),
            ..key.clone()
        },
        SetupKey {
            tool: "0.2.0".to_owned(),
            ..key.clone()
        },
    ] {
        assert_ne!(other.digest(), key.digest(), "{other:?}");
    }
}

#[test]
fn a_cache_slot_names_one_entry_and_cache_gives_back_what_no_slot_names() {
    let home = home();
    let session = home.session("work", &[], at(0));
    let (a, b, c) = (setup_key("a"), setup_key("b"), setup_key("c"));
    assert!(session.cached(&a).unwrap().is_none());

    let entry_a = session.new_entry(&a).unwrap();
    assert_eq!(
        entry_a,
        home.root()
            .join("sessions")
            .join(session.key())
            .join("cache")
            .join(a.digest())
    );
    assert!(names_in(&entry_a).is_empty());
    fs::write(entry_a.join("entry.json"), "{}").unwrap();
    assert!(
        session
            .cache(Slot::Head, &a, entry(&entry_a, "run-1"))
            .unwrap()
            .is_empty()
    );
    // The head entry of a run without `--before` is the entry before of the next comparison.
    let hit = session.cached(&a).unwrap().unwrap();
    assert_eq!((&hit.dir, hit.built_by.as_str()), (&entry_a, "run-1"));
    assert!(session.cache(Slot::Before, &a, hit).unwrap().is_empty());

    let entry_b = session.new_entry(&b).unwrap();
    assert!(
        session
            .cache(Slot::Head, &b, entry(&entry_b, "run-2"))
            .unwrap()
            .is_empty(),
        "the slot before still names the entry of a"
    );
    assert_eq!(session.cached(&a).unwrap().unwrap().dir, entry_a);
    assert_eq!(session.cached(&b).unwrap().unwrap().built_by, "run-2");
    assert!(session.cached(&c).unwrap().is_none());

    let entry_c = session.new_entry(&c).unwrap();
    assert_eq!(
        session
            .cache(Slot::Head, &c, entry(&entry_c, "run-3"))
            .unwrap(),
        vec![entry_b.clone()]
    );
    fs::write(entry_c.join("entry.json"), "{}").unwrap();
    assert_eq!(session.new_entry(&c).unwrap(), entry_c);
    assert!(names_in(&entry_c).is_empty(), "a new entry starts empty");

    home.store.clear_unused_entries().unwrap();
    assert!(!entry_b.exists());
    assert!(entry_a.join("entry.json").is_file() && entry_c.is_dir());

    let state = json_of(
        &home
            .root()
            .join("sessions")
            .join(session.key())
            .join("session.json"),
    );
    assert_eq!(
        state["cache"]["head"],
        json!({
            "key": {
                "tree": "c",
                "recipe": "r".repeat(64),
                "images": [["node:22", "sha256:1"], ["postgres:17", "sha256:2"]],
                "guest": "0.1.0",
                "tool": "0.1.0",
            },
            "digest": c.digest(),
            "builtBy": "run-3",
        })
    );
    assert_eq!(state["cache"]["before"]["builtBy"], "run-1");
}

#[test]
fn cache_entries_skip_expired_sessions_and_drop_cache_forgets_by_scope() {
    let home = home();
    let old = home.session("old", &[], at(0));
    let mine = home.session("mine", &[], at(0) + 2 * DAY);
    let theirs = home.session("theirs", &[], at(0) + 2 * DAY);
    let mut entries = Vec::new();
    for (session, tree) in [(&old, "old"), (&mine, "mine"), (&theirs, "theirs")] {
        let key = setup_key(tree);
        let dir = session.new_entry(&key).unwrap();
        session
            .cache(Slot::Before, &key, entry(&dir, "run-1"))
            .unwrap();
        entries.push(dir);
    }
    let [old_entry, my_entry, their_entry] = <[Utf8PathBuf; 3]>::try_from(entries).unwrap();

    let live: BTreeSet<Utf8PathBuf> = home
        .store
        .cache_entries(at(0) + 3 * DAY)
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(
        live,
        BTreeSet::from([my_entry.clone(), their_entry.clone()])
    );

    let dropped: BTreeSet<Utf8PathBuf> = home
        .store
        .drop_cache(KeptScope::OtherThan(mine.key().to_owned()))
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(
        dropped,
        BTreeSet::from([old_entry.clone(), their_entry.clone()])
    );
    assert!(mine.cached(&setup_key("mine")).unwrap().is_some());
    assert!(theirs.cached(&setup_key("theirs")).unwrap().is_none());
    assert!(their_entry.is_dir(), "the folder goes after its snapshots");

    assert_eq!(
        home.store.drop_cache(KeptScope::All).unwrap(),
        vec![my_entry.clone()]
    );
    assert!(mine.cached(&setup_key("mine")).unwrap().is_none());
    home.store.clear_unused_entries().unwrap();
    for entry in [old_entry, my_entry, their_entry] {
        assert!(!entry.exists(), "{entry}");
    }
}
