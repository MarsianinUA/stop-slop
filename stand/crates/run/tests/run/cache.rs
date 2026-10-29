//! The setup cache: the entry a build writes and a hit that starts from it.

use std::fs;

use camino::Utf8Path;
use report::{BeforeOutcome, Phase};
use run::{RunError, SetupCache, Side, StandFault, Version, Versions};
use sandbox::{Call, ExecOutcome, Mount, SandboxId, Script, Seed, SnapshotId};

use super::{
    Case, EXAMPLE, Hooked, SECOND, assert_clean, created, example_rules, fake, files_in, names_in,
    out_of, phase_of, probe, services, setup_snapshots, stops, with,
};

/// `side` of `case` on a hit of the entry `built` made.
fn hit(case: &Case, built: &Case, side: Side) -> Version {
    Version {
        tar: case.tar(side),
        setup: SetupCache::Hit(built.entry(side)),
    }
}

/// The snapshot each `db` of the measured repeats of `side` of `case` starts from.
fn repeat_seeds(case: &Case, journal: &[Call], side: Side) -> Vec<SnapshotId> {
    let mut seeds = Vec::new();
    for (launch, _) in created(journal) {
        let folder = out_of(&launch).and_then(Utf8Path::parent);
        let Some(folder) =
            folder.filter(|folder| folder.parent() == Some(case.run_dir().as_path()))
        else {
            continue;
        };
        let name = folder.file_name().unwrap();
        if launch.service == "db"
            && name.starts_with("repeat-")
            && name.ends_with(&side.to_string())
        {
            match &launch.seed {
                Some(Seed::Snapshot(id)) => seeds.push(id.clone()),
                other => panic!("{name}: {other:?}"),
            }
        }
    }
    seeds
}

#[test]
fn a_build_writes_out_and_entry_json_after_the_whole_setup_and_nothing_else() {
    // `db` leaves a folder in `/stand/out` beside the output of `schema`.
    let case = Case::new(EXAMPLE);
    let mut rules = example_rules();
    let db = rules.iter_mut().find(|rule| rule.service == "db").unwrap();
    let rows = ("/stand/out/dump/rows.json".to_owned(), b"[]".to_vec());
    db.script.writes.push(rows);
    let fake = fake(rules);
    case.run(&fake, &case.versions(true)).unwrap();
    let journal = fake.journal();
    for side in [Side::Before, Side::Head] {
        let entry = case.entry(side);
        assert_eq!(names_in(&entry), ["entry.json", "out"], "{side}");
        let copied = files_in(&entry.join("out"));
        let names: Vec<&str> = copied.keys().map(String::as_str).collect();
        let schema = ["probe.schema.jsonl", "probe.schema.jsonl.err"];
        assert_eq!(names, [&["dump/rows.json"][..], &schema].concat());
        let out = case.run_dir().join(format!("setup-{side}/out"));
        assert_eq!(copied, files_in(&out), "{side}");
        // Read back, the ids are the ones the port gave.
        let (db, version) = setup_snapshots(&case, &journal, side);
        let ids = run::cached_ids(&entry).unwrap().unwrap();
        assert!(
            ids.len() == 2 && ids.contains(&db) && ids.contains(&version),
            "{ids:?}"
        );
    }
    // The VMs mount nothing of an entry: setup binds `/stand/out` in the run folder, as without a
    // cache.
    let entries = [case.entry(Side::Head), case.entry(Side::Before)];
    for (launch, _) in created(&journal) {
        for mount in &launch.mounts {
            if let Mount::Bind { host, .. } = mount {
                assert!(
                    !entries.iter().any(|entry| host.starts_with(entry)),
                    "{host}"
                );
            }
        }
    }
    assert_eq!(run::cached_ids(&case.root.join("recipe")).unwrap(), None);
    assert_clean(&fake, &case.left());

    // `entry.json` comes once the whole setup is over: no setup VM sees it, up to its `destroy`;
    // the first VM of the repeats does.
    let case = Case::new(EXAMPLE);
    let fake = crate::fake(example_rules());
    let sandbox = Hooked {
        entry: Some(case.entry(Side::Head).join("entry.json")),
        ..Hooked::new(&fake)
    };
    sandbox.run(&case, &case.versions(false)).unwrap();
    let made = created(&fake.journal());
    let setup: Vec<&SandboxId> = made[..3].iter().map(|(_, id)| id).collect();
    let seen = sandbox.seen.borrow();
    assert!(
        seen[..6]
            .iter()
            .all(|(id, there)| setup.contains(&id) && !there),
        "{seen:?}"
    );
    assert_eq!(seen[6], (made[3].1.clone(), true));
    assert_clean(&fake, &case.left());
}

/// A link a service leaves in `/stand/out` is never followed: a copy would read what it points at
/// on the host. One with another name, or below the top, stays out of an entry and out of a hit,
/// as the report never reads it; a top-level `*.jsonl` one, which the report refuses, is a stand
/// fault in a build and on a hit alike.
#[cfg(unix)]
#[test]
fn a_link_in_the_setup_outputs_is_skipped_unless_it_is_named_as_a_result() {
    use std::os::unix::fs::symlink;

    let refused = |result: &Result<_, RunError>, link: &Utf8Path| {
        matches!(
            result,
            Err(RunError::Stand {
                phase: Phase::Setup,
                fault: StandFault::Io { path, .. },
            }) if *path == link
        )
    };
    // Links with another name or in a folder: the build copies the rest, and so does a hit.
    let first = Case::new(EXAMPLE);
    let out = first.run_dir().join("setup-head/out");
    fs::create_dir_all(out.join("logs")).unwrap();
    let secret = first.root.join("secret");
    fs::write(&secret, b"host").unwrap();
    symlink(&secret, out.join("leak")).unwrap();
    symlink(&secret, out.join("logs/leak.jsonl")).unwrap();
    let fake = fake(example_rules());
    first.run(&fake, &first.versions(false)).unwrap();
    let entry_out = first.entry(Side::Head).join("out");
    let copied = ["logs", "probe.schema.jsonl", "probe.schema.jsonl.err"];
    assert_eq!(names_in(&entry_out), copied);
    assert!(names_in(&entry_out.join("logs")).is_empty());
    assert_clean(&fake, &first.left());
    symlink(&secret, entry_out.join("leak")).unwrap();
    let second = Case::new(EXAMPLE);
    let versions = Versions {
        head: hit(&second, &first, Side::Head),
        before: None,
    };
    second.run(&fake, &versions).unwrap();
    assert_eq!(names_in(&second.run_dir().join("setup-head/out")), copied);
    let mut left = first.left();
    left.extend(second.kept());
    assert_clean(&fake, &left);

    // A top-level `*.jsonl` link in a build: its setup passes, no entry is written, no repeat
    // starts, its snapshots go.
    let case = Case::new(EXAMPLE);
    let out = case.run_dir().join("setup-head/out");
    fs::create_dir_all(&out).unwrap();
    symlink(&secret, out.join("leak.jsonl")).unwrap();
    let fake = crate::fake(example_rules());
    let result = case.run(&fake, &case.versions(false));
    assert!(refused(&result, &out.join("leak.jsonl")), "{result:?}");
    assert_eq!(run::cached_ids(&case.entry(Side::Head)).unwrap(), None);
    assert_eq!(
        services(&fake.journal()),
        ["db-1", "prepare-1", "migrate-1"]
    );
    assert_clean(&fake, &[]);

    // A hit on an entry that holds one: nothing starts.
    let first = Case::new(EXAMPLE);
    let fake = crate::fake(example_rules());
    first.run(&fake, &first.versions(false)).unwrap();
    let built = fake.journal().len();
    let link = first.entry(Side::Head).join("out/leak.jsonl");
    symlink(&secret, &link).unwrap();
    let second = Case::new(EXAMPLE);
    let versions = Versions {
        head: hit(&second, &first, Side::Head),
        before: None,
    };
    let result = second.run(&fake, &versions);
    assert!(refused(&result, &link), "{result:?}");
    assert_eq!(fake.journal().len(), built);
    assert_clean(&fake, &first.left());
}

#[test]
fn a_hit_starts_no_setup_vm_and_holds_after_the_run_folder_that_built_it_is_gone() {
    // `db` binds the harness too, and `schema` exits 2: a hit gives that outcome back.
    let recipe = EXAMPLE.replace(
        r#"volumes: ["pgdata:/var/lib/postgresql/data"]"#,
        r#"volumes: ["pgdata:/var/lib/postgresql/data", ".:/harness:ro"]"#,
    );
    let db = Script {
        volume_bytes: 4096,
        probes: vec![
            (
                "probe.schema.jsonl".into(),
                ExecOutcome::Exited(2),
                b"half\n".to_vec(),
            ),
            probe("probe.orders.jsonl", b"{\"id\":1}\n"),
        ],
        ..stops(SECOND)
    };
    let first = Case::new(&recipe);
    let fake = fake(with("db", db));
    first.run(&fake, &first.versions(true)).unwrap();
    let built = fake.journal();
    let seeds = [Side::Before, Side::Head].map(|side| setup_snapshots(&first, &built, side));
    // Of the first run only its entries and its kept snapshots stay: `cli` drops the kept ones
    // once a later run keeps its own.
    let mut left = first.left();
    fs::remove_dir_all(first.run_dir()).unwrap();
    fs::remove_dir_all(first.root.join("recipe")).unwrap();

    let second = Case::new(&recipe);
    let versions = Versions {
        head: hit(&second, &first, Side::Head),
        before: Some(hit(&second, &first, Side::Before)),
    };
    let raw = second.run(&fake, &versions).unwrap();
    let journal = fake.journal();
    let later = &journal[built.len()..];
    // Only repeats: three of each version and the coverage repeat of the head.
    let repeat = ["db-1", "api-1", "api-2", "load-1"];
    assert_eq!(services(later), repeat.repeat(2 * 3 + 1));
    // Each from the entry of its version: `db` from its setup snapshot with the harness of this
    // run, `api` from the version snapshot.
    for vms in created(later).chunks(4) {
        let (db, _) = &vms[0];
        let folder = phase_of(db);
        let (setup, version) = &seeds[usize::from(!folder.ends_with("before"))];
        assert!(
            matches!(&db.seed, Some(Seed::Snapshot(id)) if id == setup),
            "{folder}"
        );
        for (api, _) in &vms[1..3] {
            assert!(matches!(&api.seed, Some(Seed::Snapshot(id)) if id == version));
        }
        let harness = db.mounts.iter().find_map(|mount| match mount {
            Mount::Bind { host, guest, .. } if guest == "/harness" => Some(host),
            _ => None,
        });
        assert!(harness.is_some_and(|host| host.starts_with(&second.root)));
    }
    let setups = [
        (Side::Head, &raw.setup.head),
        (Side::Before, raw.setup.before.as_ref().unwrap()),
    ];
    for (side, setup) in setups {
        let out = second.run_dir().join(format!("setup-{side}/out"));
        assert_eq!(setup.out_dir, out);
        assert_eq!(setup.sizes, [("pgdata".to_owned(), 4096)].into());
        let probes: Vec<(&str, ExecOutcome, &Utf8Path)> = setup
            .probes
            .iter()
            .map(|probe| (probe.name.as_str(), probe.outcome, probe.out.as_path()))
            .collect();
        let schema = out.join("probe.schema.jsonl");
        assert_eq!(
            probes,
            [("schema", ExecOutcome::Exited(2), schema.as_path())]
        );
        assert_eq!(fs::read(&schema).unwrap(), b"half\n");
        assert_eq!(files_in(&out), files_in(&first.entry(side).join("out")));
        assert_eq!(
            names_in(out.parent().unwrap()),
            ["out"],
            "no setup ran in {side}"
        );
    }
    assert!(matches!(&raw.before, Some(BeforeOutcome::Passed(before)) if before.len() == 3));
    left.extend(second.kept());
    assert_clean(&fake, &left);

    // The head built anew beside a hit of "before", as in the live check: one setup runs, the
    // head's, and its entry stays beside the first ones.
    let third = Case::new(&recipe);
    let versions = Versions {
        head: third.version(Side::Head),
        before: Some(hit(&third, &first, Side::Before)),
    };
    let mark = fake.journal().len();
    let raw = third.run(&fake, &versions).unwrap();
    let journal = fake.journal();
    let names = services(&journal[mark..]);
    assert_eq!(&names[..3], ["db-1", "prepare-1", "migrate-1"]);
    let setup = names.iter().filter(|name| !repeat.contains(&name.as_str()));
    assert_eq!(setup.count(), 2, "one prepare and one migrate");
    let before = repeat_seeds(&third, &journal[mark..], Side::Before);
    assert_eq!(
        before,
        vec![seeds[0].0.clone(); 3],
        "from the entry of the first run"
    );
    let sizes = &raw.setup.before.as_ref().unwrap().sizes;
    assert_eq!(*sizes, [("pgdata".to_owned(), 4096)].into());
    left.extend(third.left());
    assert_eq!(
        left.len(),
        3 * 2 + 3 * 2,
        "three entries of two snapshots, three runs that kept `db` of each side"
    );
    assert_clean(&fake, &left);
}

#[test]
fn a_fault_in_writing_or_reading_an_entry_is_a_stand_fault_in_setup() {
    // The entry of the head is a file: its setup passes, the entry cannot be written, no repeat
    // starts, and the snapshots of that setup go.
    let case = Case::new(EXAMPLE);
    let entry = case.entry(Side::Head);
    fs::remove_dir(&entry).unwrap();
    fs::write(&entry, b"").unwrap();
    let fake = fake(example_rules());
    let result = case.run(&fake, &case.versions(false));
    assert!(
        matches!(
            &result,
            Err(RunError::Stand {
                phase: Phase::Setup,
                fault: StandFault::Io { path, .. },
            }) if path.starts_with(&entry)
        ),
        "{result:?}"
    );
    assert_eq!(
        services(&fake.journal()),
        ["db-1", "prepare-1", "migrate-1"]
    );
    assert_clean(&fake, &[]);

    // A hit on an entry without `entry.json`, then on one that does not read: nothing starts.
    let case = Case::new(EXAMPLE);
    let fake = crate::fake(example_rules());
    let versions = Versions {
        head: hit(&case, &case, Side::Head),
        before: None,
    };
    let file = case.entry(Side::Head).join("entry.json");
    for bytes in [None, Some(b"{".as_slice())] {
        if let Some(bytes) = bytes {
            fs::write(&file, bytes).unwrap();
        }
        let result = case.run(&fake, &versions);
        assert!(
            matches!(
                &result,
                Err(RunError::Stand {
                    phase: Phase::Setup,
                    fault: StandFault::Io { path, .. },
                }) if *path == file
            ),
            "{result:?}"
        );
    }
    assert!(fake.journal().is_empty());
    let read = run::cached_ids(&case.entry(Side::Head));
    assert!(
        matches!(&read, Err(StandFault::Io { path, .. }) if *path == file),
        "{read:?}"
    );
}
