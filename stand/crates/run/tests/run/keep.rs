//! The end-of-run snapshots, `kept.json`, and `inspect` on them.

use std::fs;
use std::time::Duration;

use camino::Utf8Path;
use report::{BeforeOutcome, Phase};
use run::{Cancel, InspectError, RunError, Side, StandFault};
use sandbox::{Call, ExecOutcome, Mount, Rule, Sandbox, Script, Seed};

use super::{
    Case, EXAMPLE, Hooked, SECOND, app_failure, assert_clean, assert_left, created, example_rules,
    exits, fake, index_of, kept_from, probe, profile_mounts, rule, stops, with,
};

#[test]
fn the_last_measured_repeat_of_each_version_keeps_each_volume_service() {
    let case = Case::new(EXAMPLE);
    let fake = fake(example_rules());
    case.run(&fake, &case.versions(true)).unwrap();
    let journal = fake.journal();
    assert_eq!(
        kept_from(&case, &journal),
        [
            ["before", "db", "repeat-3-before"],
            ["head", "db", "repeat-3-head"]
        ]
    );
    // Only `db` has a volume, and no other repeat, the coverage repeat neither, takes a snapshot:
    // setup takes `db` and the version of each side.
    let snapshots = journal
        .iter()
        .filter(|call| matches!(call, Call::Snapshot { .. }));
    assert_eq!(snapshots.count(), 2 * 2 + 2);
    // Each after the stop and the sizes of its VM, before its `destroy`.
    let ids = case.kept();
    assert_eq!(ids.len(), 2);
    for id in &ids {
        let taken = index_of(
            &journal,
            |call| matches!(call, Call::Snapshot { snapshot: Some(own), .. } if own == id),
        );
        let Call::Snapshot { id: vm, .. } = &journal[taken] else {
            unreachable!()
        };
        let at = |matches: &dyn Fn(&Call) -> bool| index_of(&journal, matches);
        let stopped = at(&|call| matches!(call, Call::Stop(made) if made == vm));
        let sized = at(&|call| matches!(call, Call::VolumeSizes(made) if made == vm));
        let gone = at(&|call| matches!(call, Call::Destroy(made) if made == vm));
        assert!(stopped < sized && sized < taken && taken < gone, "{id}");
    }
    // camelCase, with what `inspect` needs of the kept repeat.
    let text = fs::read_to_string(case.run_dir().join("kept.json")).unwrap();
    let kept: serde_json::Value = serde_json::from_str(&text).unwrap();
    let head = &kept[1];
    assert_eq!(head["snapshot"], ids[1].as_str());
    let launch = &head["launch"];
    assert_eq!(
        (&launch["stopSignal"], &launch["stopGraceMs"]),
        (&serde_json::json!("SIGINT"), &serde_json::json!(60_000))
    );
    assert_eq!(launch["health"]["test"], serde_json::json!(["pg_isready"]));
    let out = case.run_dir().join("repeat-3-head").join("out");
    let mounts = serde_json::json!([
        { "kind": "volume", "name": "pgdata", "guest": "/var/lib/postgresql/data" },
        { "kind": "bind", "host": out.as_str(), "guest": "/stand/out", "readOnly": false },
    ]);
    assert_eq!(launch["mounts"], mounts);
    assert_clean(&fake, &case.left());
    assert!(
        run::kept_ids(&case.root.join("nowhere"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_failed_head_repeat_keeps_its_snapshot_and_failed_kept_names_it() {
    // The workload of both versions fails in the first repeat: each keeps `db` of its failure, and
    // `Failed.kept` names only the head's.
    let case = Case::new(EXAMPLE);
    let fake = fake(with("load", exits(1, SECOND)));
    let failed = app_failure(case.run(&fake, &case.versions(true)));
    assert_eq!(failed.kept, ["db"]);
    assert_eq!(
        kept_from(&case, &fake.journal()),
        [
            ["before", "db", "repeat-1-before"],
            ["head", "db", "repeat-1-head"]
        ]
    );
    assert_clean(&fake, &case.left());

    // The head fails in its second repeat, before which "before" passed two: the run ends there,
    // and "before" has no last repeat to keep.
    let case = Case::new(EXAMPLE);
    let fake = crate::fake(example_rules());
    let sandbox = Hooked {
        hang: Some(("load", 4)),
        ..Hooked::new(&fake)
    };
    let failed = app_failure(sandbox.run(&case, &case.versions(true)));
    assert_eq!(failed.failure.repeat, Some(2));
    assert_eq!(failed.kept, ["db"]);
    assert_eq!(
        kept_from(&case, &fake.journal()),
        [["head", "db", "repeat-2-head"]]
    );
    assert!(matches!(&failed.raw.before, Some(BeforeOutcome::Passed(before)) if before.len() == 2));
    assert_clean(&fake, &case.left());
}

#[test]
fn a_kept_json_that_cannot_be_written_breaks_the_stand_and_leaves_no_snapshot() {
    // `kept.json` is a folder: a run that passed is a stand fault at its end, a failed head names
    // no service to inspect; the snapshots no list names go.
    for rules in [example_rules(), with("load", exits(1, SECOND))] {
        let case = Case::new(EXAMPLE);
        let kept = case.run_dir().join("kept.json");
        fs::create_dir(&kept).unwrap();
        let fake = fake(rules);
        match case.run(&fake, &case.versions(false)) {
            Err(RunError::Stand {
                phase: Phase::Workload,
                fault: StandFault::Io { path, .. },
            }) => assert_eq!(path, kept),
            Err(RunError::App(failed)) => assert!(failed.kept.is_empty()),
            other => panic!("{other:?}"),
        }
        assert_clean(&fake, &case.cached());
    }
}

/// `db` with a volume and a probe; `queue` with a volume, the harness, a profile and no
/// healthcheck. The one repeat keeps both.
const KEPT: &str = r#"
services:
  db:
    image: postgres:17
    volumes: ["pgdata:/var/lib/postgresql/data"]
    expose: [5432]
    healthcheck: { test: ["CMD", "pg_isready"], interval: 1s, retries: 2 }
    deploy: { resources: { limits: { cpus: "1", memory: 1g } } }
  queue:
    image: node:22
    volumes: ["jobs:/var/jobs", ".:/harness:ro"]
    command: ["node", "/harness/queue.js"]
    environment: { NODE_OPTIONS: "--max-old-space-size=256" }
    expose: [4000]
    deploy: { resources: { limits: { cpus: "1", memory: 512m } } }
    x-stand: { profile: node }
  load:
    image: node:22
    depends_on:
      db: { condition: service_healthy }
      queue: { condition: service_started }
    deploy: { resources: { limits: { cpus: "1", memory: 512m } } }
volumes: { pgdata: {}, jobs: {} }
x-stand:
  workload: load
  repeat: 1
  probes:
    rows: { in: db, at: after-workload, run: ["psql", "-c", "select 1"] }
"#;

fn kept_rules(db: Script, queue: Script) -> Vec<Rule> {
    vec![
        rule("db", db),
        rule("queue", queue),
        rule("load", exits(0, 5 * SECOND)),
    ]
}

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

#[test]
fn inspect_restores_the_kept_snapshot_without_peers_or_network_and_destroys_it() {
    let case = Case::new(KEPT);
    let db = Script {
        probes: vec![
            probe("1.out", b"3\n"),
            ("2.out".into(), ExecOutcome::Exited(1), Vec::new()),
        ],
        ..stops(SECOND)
    };
    let fake = fake(kept_rules(db, stops(SECOND)));
    case.run(&fake, &case.versions(false)).unwrap();
    let kept = case.kept();
    let mark = fake.journal().len();
    let sandbox = Hooked::new(&fake);
    let inspect = |service: &str, words: &[&str]| {
        let looked = run::inspect(
            &sandbox,
            &case.run_dir(),
            Side::Head,
            service,
            &argv(words),
            &Cancel::new(),
        );
        looked.unwrap()
    };
    let dir = case.run_dir().join("inspect");
    let looked = inspect("db", &["psql", "-c", "select count(*) from orders"]);
    assert_eq!(looked.outcome, ExecOutcome::Exited(0));
    assert_eq!(looked.out, dir.join("1.out"));
    assert_eq!(fs::read(&looked.out).unwrap(), b"3\n");
    // The next look writes the next file; a code other than 0 is what its command gave.
    let again = inspect("db", &["false"]);
    assert_eq!(
        (again.outcome, again.out),
        (ExecOutcome::Exited(1), dir.join("2.out"))
    );
    let queue = inspect("queue", &["ls", "/var/jobs"]);
    assert_eq!(queue.out, dir.join("3.out"));
    assert_eq!(*sandbox.deadlines.borrow(), [60 * SECOND; 3]);
    // Each look is one VM from its kept snapshot with the launch of the kept repeat, but no peers,
    // no ports, no network and `/stand/out` on `inspect/`; it is ready first, runs `argv`, goes.
    let journal = fake.journal();
    let made = created(&journal[..mark]);
    let looks = created(&journal[mark..]);
    let expected = [("db", &kept[0]), ("db", &kept[0]), ("queue", &kept[1])];
    assert_eq!(looks.len(), expected.len());
    for ((launch, id), (service, snapshot)) in looks.iter().zip(expected) {
        let taken = journal.iter().find_map(|call| match call {
            Call::Snapshot {
                id,
                snapshot: Some(own),
            } if own == snapshot => Some(id),
            _ => None,
        });
        let (repeat, _) = made.iter().find(|(_, made)| Some(made) == taken).unwrap();
        assert_eq!(repeat.service, service);
        let mut restored = repeat.clone();
        restored.peers = Vec::new();
        restored.ports = Vec::new();
        restored.outbound = sandbox::Outbound::Off;
        restored.seed = Some(Seed::Snapshot(snapshot.clone()));
        restored
            .mounts
            .retain(|mount| !matches!(mount, Mount::Bind { guest, .. } if guest == "/stand/out"));
        restored.mounts.push(Mount::Bind {
            host: dir.clone(),
            guest: "/stand/out".into(),
            read_only: false,
        });
        assert_eq!(format!("{launch:?}"), format!("{restored:?}"), "{service}");
        let on = |matches: &dyn Fn(&Call) -> bool| index_of(&journal, matches);
        let ran = on(&|call| matches!(call, Call::Exec { id: made, .. } if made == id));
        let gone = on(&|call| matches!(call, Call::Destroy(made) if made == id));
        assert!(ran < gone, "{service}");
        let checked = journal
            .iter()
            .position(|call| matches!(call, Call::ExecDetached { id: made, .. } if made == id));
        assert_eq!(
            checked.is_some_and(|checked| checked < ran),
            service == "db"
        );
    }
    // `queue` binds the harness and the preload of the run again, and has its chunks folder.
    let (queue, _) = &looks[2];
    let binds: Vec<(&str, &Utf8Path, bool)> = queue
        .mounts
        .iter()
        .filter_map(|mount| match mount {
            Mount::Bind {
                host,
                guest,
                read_only,
            } => Some((guest.as_str(), host.as_path(), *read_only)),
            _ => None,
        })
        .collect();
    let preload = case.run_dir().join("preload");
    let recipe = case.root.join("recipe");
    assert_eq!(
        binds,
        [
            ("/harness", recipe.as_path(), true),
            ("/stand/preload", preload.as_path(), true),
            ("/stand/out", dir.as_path(), false)
        ]
    );
    assert_eq!(profile_mounts(queue), (Some(preload.as_path()), true));
    // Nothing reaches the snapshots.
    let later = &journal[mark..];
    assert!(
        !later
            .iter()
            .any(|call| matches!(call, Call::Snapshot { .. } | Call::DropSnapshot(_)))
    );
    assert_left(&fake, &case.left());
}

#[test]
fn inspect_refuses_what_is_not_kept_and_a_gone_harness_folder_creates_nothing() {
    let case = Case::new(KEPT);
    let fake = fake(kept_rules(stops(SECOND), stops(SECOND)));
    let cancel = Cancel::new();
    let inspect = |run_dir: &Utf8Path, side: Side, service: &str| {
        run::inspect(&fake, run_dir, side, service, &argv(&["true"]), &cancel)
    };
    // A run folder without `kept.json`, none at all, a side, a service without a volume, no such
    // service.
    let refused = inspect(&case.run_dir(), Side::Head, "db");
    assert!(
        matches!(refused, Err(InspectError::NotKept(_))),
        "{refused:?}"
    );
    case.run(&fake, &case.versions(false)).unwrap();
    let left = case.left();
    let mark = fake.journal().len();
    let (nowhere, run_dir) = (case.root.join("nowhere"), case.run_dir());
    let asked = [
        (nowhere.as_path(), Side::Head, "db"),
        (run_dir.as_path(), Side::Before, "db"),
        (run_dir.as_path(), Side::Head, "load"),
        (run_dir.as_path(), Side::Head, "api"),
    ];
    for (run_dir, side, service) in asked {
        let refused = inspect(run_dir, side, service);
        assert!(
            matches!(&refused, Err(InspectError::NotKept(_))),
            "{service}: {refused:?}"
        );
    }
    // The harness folder is gone, then the preload of the run: `NotStarted` with its path, and no
    // VM; `db`, which binds neither, still starts.
    let recipe = case.root.join("recipe");
    let preload = case.run_dir().join("preload");
    fs::remove_dir_all(&recipe).unwrap();
    for gone in [&recipe, &preload] {
        if *gone == preload {
            fs::create_dir(&recipe).unwrap();
            fs::remove_dir_all(&preload).unwrap();
        }
        let made = fake.journal().len();
        match inspect(&case.run_dir(), Side::Head, "queue") {
            Err(InspectError::NotStarted { service, cause }) => {
                assert_eq!(service, "queue");
                assert_eq!(cause, format!("its folder {gone} is gone"));
            }
            other => panic!("{other:?}"),
        }
        assert!(created(&fake.journal()[made..]).is_empty(), "{gone}");
    }
    inspect(&case.run_dir(), Side::Head, "db").unwrap();
    assert_eq!(created(&fake.journal()[mark..]).len(), 1);
    assert_left(&fake, &left);
    // A `kept.json` that does not read breaks the stand.
    let file = case.run_dir().join("kept.json");
    fs::write(&file, "[").unwrap();
    let refused = inspect(&case.run_dir(), Side::Head, "db");
    assert!(
        matches!(&refused, Err(InspectError::Stand(StandFault::Io { path, .. })) if *path == file),
        "{refused:?}"
    );
    let read = run::kept_ids(&case.run_dir());
    assert!(
        matches!(&read, Err(StandFault::Io { path, .. }) if *path == file),
        "{read:?}"
    );
}

#[test]
fn inspect_of_a_service_that_exits_or_stays_unhealthy_did_not_start_and_cancel_destroys_it() {
    // Each head fails in its repeat, which keeps both; the restored VM meets the same script:
    // `queue` exits at once, `db` fails its healthcheck, `db`'s attempts hang past 60 s.
    let exits_at_once = Script {
        exit: Some((1, Duration::ZERO)),
        ..stops(SECOND)
    };
    let unhealthy = Script {
        health: vec![Some(1)],
        ..stops(SECOND)
    };
    let hanging = Script {
        health: vec![None],
        ..stops(SECOND)
    };
    let cases = [
        (stops(SECOND), exits_at_once, "queue", "it exited with 1"),
        (unhealthy, stops(SECOND), "db", "it stayed unhealthy"),
        (
            hanging,
            stops(SECOND),
            "db",
            "it was not healthy within 60 s",
        ),
    ];
    for (db, queue, service, cause) in cases {
        let case = Case::new(KEPT);
        let fake = fake(kept_rules(db, queue));
        let failed = app_failure(case.run(&fake, &case.versions(false)));
        assert_eq!(failed.kept, ["db", "queue"]);
        let mark = fake.journal().len();
        let start = fake.now();
        let refused = run::inspect(
            &fake,
            &case.run_dir(),
            Side::Head,
            service,
            &argv(&["true"]),
            &Cancel::new(),
        );
        let Err(InspectError::NotStarted {
            service: own,
            cause: said,
        }) = refused
        else {
            panic!("{service}: {refused:?}")
        };
        assert_eq!(own, service);
        assert_eq!(said, format!("{cause}, without its peers and network"));
        if cause.contains("60 s") {
            assert_eq!(fake.now() - start, 60 * SECOND);
        }
        let later = &fake.journal()[mark..];
        assert_eq!(created(later).len(), 1, "{service}");
        assert!(!later.iter().any(|call| matches!(call, Call::Exec { .. })));
        assert_left(&fake, &case.left());
    }

    // Ctrl+C once the VM is created, with a healthcheck and without: it goes, and nothing runs
    // in it; set before, no VM starts.
    let case = Case::new(KEPT);
    let fake = fake(kept_rules(stops(SECOND), stops(SECOND)));
    case.run(&fake, &case.versions(false)).unwrap();
    for (service, at_create) in [("db", true), ("queue", true), ("db", false)] {
        let sandbox = Hooked {
            cancel_at: at_create.then_some(service),
            ..Hooked::new(&fake)
        };
        if !at_create {
            sandbox.cancel.cancel();
        }
        let mark = fake.journal().len();
        let refused = run::inspect(
            &sandbox,
            &case.run_dir(),
            Side::Head,
            service,
            &argv(&["true"]),
            &sandbox.cancel,
        );
        assert!(
            matches!(refused, Err(InspectError::Cancelled)),
            "{service}: {refused:?}"
        );
        let later = &fake.journal()[mark..];
        assert_eq!(created(later).len(), usize::from(at_create), "{service}");
        assert!(!later.iter().any(|call| matches!(call, Call::Exec { .. })));
        assert_left(&fake, &case.left());
    }
}
