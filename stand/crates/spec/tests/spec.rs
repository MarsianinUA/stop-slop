use std::collections::BTreeMap;
use std::fs;
use std::time::Duration;

use camino::Utf8Path;
use spec::{
    Change, Condition, Demand, FaultMode, Mount, Outbound, Policy, PolicyError, Profiler, Recipe,
    Service, ServiceKind, ServiceName, SpecError, Stand, Tap, load,
};

mod common;

use common::{assert_refused, fixture, load_case};

fn stand(case: &str) -> Stand {
    stand_at(&fixture(case))
}

fn stand_at(file: &Utf8Path) -> Stand {
    load(file, &Policy::new(None).unwrap()).unwrap_or_else(|error| panic!("{file}: {error}"))
}

fn service<'a>(stand: &'a Stand, name: &str) -> &'a Service {
    stand
        .services
        .iter()
        .find(|(known, _)| known.as_str() == name)
        .map(|(_, service)| service)
        .unwrap_or_else(|| panic!("no service {name}"))
}

fn ports(service: &Service) -> Vec<u16> {
    service.expose.iter().map(|port| port.get()).collect()
}

/// `interval`, `timeout`, `retries` and `start_period`.
fn health(service: &Service) -> (Duration, Duration, u32, Duration) {
    let health = service.healthcheck.as_ref().unwrap();
    (
        health.interval,
        health.timeout,
        health.retries,
        health.start_period,
    )
}

fn waits(service: &Service) -> Vec<(&str, Condition)> {
    service
        .depends_on
        .iter()
        .map(|(name, condition)| (name.as_str(), *condition))
        .collect()
}

/// `tree`, `harness <path from the fixture folder> <target>` and `volume <name> <target>`,
/// with ` ro` when read-only.
fn mounts(stand: &Stand, name: &str) -> Vec<String> {
    let dir = stand.file.parent().unwrap();
    let ro = |read_only: bool| if read_only { " ro" } else { "" };
    service(stand, name)
        .mounts
        .iter()
        .map(|mount| match mount {
            Mount::Tree => "tree".to_owned(),
            Mount::Harness {
                source,
                target,
                read_only,
            } => {
                let source = source.strip_prefix(dir).unwrap().as_str();
                let source = if source.is_empty() { "." } else { source };
                format!("harness {source} {target}{}", ro(*read_only))
            }
            Mount::Volume {
                name,
                target,
                read_only,
            } => format!("volume {name} {target}{}", ro(*read_only)),
        })
        .collect()
}

#[test]
fn the_example_of_the_plan_reads_into_kinds_levels_defaults_and_the_prepare_service() {
    let stand = stand("example");
    assert!(stand.file.is_absolute());
    let kinds: Vec<(&str, ServiceKind)> = stand
        .services
        .iter()
        .map(|(name, service)| (name.as_str(), service.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("api", ServiceKind::LongRunning),
            ("db", ServiceKind::LongRunning),
            ("load", ServiceKind::Workload),
            ("migrate", ServiceKind::Setup),
            ("prepare", ServiceKind::Setup),
        ]
    );
    let levels: Vec<Vec<&str>> = stand
        .levels
        .iter()
        .map(|level| level.iter().map(ServiceName::as_str).collect())
        .collect();
    assert_eq!(
        levels,
        [
            vec!["db", "prepare"],
            vec!["migrate"],
            vec!["api"],
            vec!["load"]
        ]
    );
    assert_eq!(stand.workload.as_str(), "load");
    assert_eq!(
        stand.prepare.as_ref().map(ServiceName::as_str),
        Some("prepare")
    );
    assert_eq!(
        (
            stand.repeat.get(),
            stand.setup_timeout.get(),
            stand.timeout.get()
        ),
        (3, Duration::from_secs(600), Duration::from_secs(300))
    );

    assert_eq!(
        mounts(&stand, "db"),
        ["volume pgdata /var/lib/postgresql/data"]
    );
    assert_eq!(mounts(&stand, "prepare"), ["tree"]);
    assert_eq!(mounts(&stand, "api"), ["tree", "harness . /harness ro"]);
    assert_eq!(mounts(&stand, "load"), ["harness . /harness ro"]);

    let db = service(&stand, "db");
    assert_eq!(db.image, "postgres:17");
    assert_eq!(
        db.command.as_deref(),
        Some(&["postgres", "-c", "wal_level=logical"].map(String::from)[..])
    );
    assert_eq!(
        db.environment,
        BTreeMap::from([("POSTGRES_PASSWORD".to_owned(), "stand".to_owned())])
    );
    assert_eq!(ports(db), [5432]);
    assert_eq!(
        db.healthcheck.as_ref().unwrap().test,
        ["pg_isready", "-U", "postgres"]
    );
    assert_eq!(
        health(db),
        (
            Duration::from_secs(1),
            Duration::from_secs(30),
            30,
            Duration::ZERO
        )
    );
    assert_eq!((db.cpus.get(), db.memory.get()), (1000, 1 << 30));

    let migrate = service(&stand, "migrate");
    assert_eq!(
        waits(migrate),
        [
            ("db", Condition::Healthy),
            ("prepare", Condition::CompletedSuccessfully)
        ]
    );
    assert_eq!(migrate.working_dir.as_deref(), Some("/w/app"));

    for (name, service) in &stand.services {
        let name = name.as_str();
        assert_eq!(
            service.replicas.get(),
            if name == "api" { 2 } else { 1 },
            "{name}"
        );
        assert_eq!(
            (service.stop_signal.as_deref(), service.stop_grace),
            (None, Duration::from_secs(10))
        );
        let outbound = if name == "prepare" {
            Outbound::Public
        } else {
            Outbound::Off
        };
        assert_eq!(service.outbound, outbound, "{name}");
        let profile = (name == "api").then_some(Profiler::Node);
        assert_eq!(service.profile, profile, "{name}");
    }

    let probes: Vec<&str> = stand.probes.keys().map(|name| name.as_str()).collect();
    assert_eq!(probes, ["orders"]);
    let orders = stand.probes.values().next().unwrap();
    assert_eq!(
        (orders.service.as_str(), orders.at),
        ("db", Tap::AfterWorkload)
    );
    assert_eq!(orders.run[..3], ["psql", "-U", "postgres"]);
    assert_eq!(orders.key.as_ref().map(|key| key.as_str()), Some("/id"));
    let ignore: Vec<&str> = orders.ignore.iter().map(|path| path.as_str()).collect();
    assert_eq!(ignore, ["/updated_at"]);
    let [cost] = &stand.expect[..] else {
        panic!("{:?}", stand.expect);
    };
    assert_eq!(cost.measure, "stand.wall");
    assert!(matches!(cost.change, Change::UpTo(percent) if percent.get() == 10));
    assert_eq!(cost.reason, "one more index lookup per order");
    assert!(stand.fault.is_none() && stand.second_pass.is_none());
}

#[test]
fn values_take_the_forms_of_compose() {
    let stand = stand("values");
    let cache = service(&stand, "cache");
    assert_eq!(
        cache.command.as_deref(),
        Some(&["redis-server", "--save", "", "--appendonly", "no"].map(String::from)[..])
    );
    assert_eq!(
        cache.environment,
        BTreeMap::from([
            ("EMPTY".to_owned(), String::new()),
            ("MODE".to_owned(), "fast".to_owned())
        ])
    );
    assert_eq!(ports(cache), [6379]);
    assert_eq!(
        cache.healthcheck.as_ref().unwrap().test,
        ["sh", "-c", "redis-cli ping"]
    );
    assert_eq!(
        health(cache),
        (
            Duration::from_secs(30),
            Duration::from_secs(30),
            3,
            Duration::ZERO
        )
    );
    assert_eq!(
        (cache.stop_signal.as_deref(), cache.stop_grace),
        (Some("SIGINT"), Duration::from_secs(90))
    );
    assert_eq!(
        (cache.cpus.get(), cache.memory.get(), cache.replicas.get()),
        (500, 1 << 30, 3)
    );

    let worker = service(&stand, "worker");
    assert_eq!(worker.entrypoint.as_deref(), Some(&["node".to_owned()][..]));
    assert_eq!(
        worker.command.as_deref(),
        Some(&["worker.js".to_owned()][..])
    );
    // A boolean reads as Compose writes it, a number as written, also after an anchor or a tag,
    // `$$` as `$`. An alias of a number reads as YAML gives it back.
    assert_eq!(
        worker.environment,
        BTreeMap::from([
            ("AGAIN".to_owned(), "3000".to_owned()),
            ("DEBUG".to_owned(), "true".to_owned()),
            ("HALF".to_owned(), "0.50".to_owned()),
            ("NOTE".to_owned(), "naïve ☃".to_owned()),
            ("PORT".to_owned(), "+3000".to_owned()),
            ("PRICE".to_owned(), "$5".to_owned()),
            ("RATIO".to_owned(), "0.50".to_owned()),
        ])
    );
    assert_eq!(
        worker.healthcheck.as_ref().unwrap().test,
        ["sh", "-c", "test -f /tmp/ready"]
    );
    assert_eq!(
        health(worker),
        (
            Duration::from_millis(500),
            Duration::from_secs(2),
            5,
            Duration::from_secs(60)
        )
    );
    assert_eq!((worker.cpus.get(), worker.memory.get()), (500, 1 << 30));

    let load = service(&stand, "load");
    assert_eq!(
        waits(load),
        [
            ("cache", Condition::Started),
            ("worker", Condition::Started)
        ]
    );
    assert_eq!((load.cpus.get(), load.memory.get()), (2000, 512 << 20));
    assert_eq!(
        (
            stand.repeat.get(),
            stand.setup_timeout.get(),
            stand.timeout.get()
        ),
        (5, Duration::from_secs(120), Duration::from_secs(30))
    );
}

#[test]
fn a_bad_value_is_refused_at_its_key() {
    assert_refused(&[
        (
            "bad-cpus",
            "services.load.deploy.resources.limits.cpus",
            4,
            "CPUs",
        ),
        (
            "bad-memory",
            "services.load.deploy.resources.limits.memory",
            4,
            "size",
        ),
        (
            "bad-duration",
            "services.load.stop_grace_period",
            4,
            "duration",
        ),
        (
            "environment-unset",
            "services.load.environment.API_TOKEN",
            6,
            "takes none from the host",
        ),
    ]);
}

#[test]
fn demand_adds_whole_cpus_and_memory_of_every_replica() {
    assert_eq!(
        stand("example").demand(),
        Demand {
            cpus: 1 + 2 + 1 + 2 * 2 + 1,
            memory: (1 << 30) + (2 << 30) + (512 << 20) + 2 * (1 << 30) + (512 << 20),
        }
    );
    // Three replicas of 0.5 CPU take a whole CPU each.
    assert_eq!(
        stand("values").demand(),
        Demand {
            cpus: 3 + 1 + 2,
            memory: 3 * (1 << 30) + (1 << 30) + (512 << 20),
        }
    );
}

#[test]
fn the_allowlist_refuses_keys_the_stand_does_not_read() {
    assert_refused(&[
        (
            "unknown-key",
            "services.load.deploy.resources.limits.pids",
            9,
            "`cpus`, `memory`",
        ),
        ("build", "services.load.build", 4, "name it in `image`"),
        ("ports", "services.load.ports", 4, "`expose`"),
        (
            "extension-outside",
            "services.load.deploy.x-notes",
            6,
            "top level or in a service",
        ),
        ("syntax", "yaml", 5, ""),
        ("second-document", "yaml", 7, "a second document"),
        ("mapping-key", "yaml", 4, "not a name"),
        ("duplicate-key", "yaml", 4, "duplicated key"),
        (
            "merge-key",
            "services.load.environment.<<",
            5,
            "merge keys are not supported",
        ),
        (
            "wrong-type",
            "services.load.healthcheck",
            4,
            "expected a mapping, found `curl",
        ),
    ]);
    stand("extension");
}

#[test]
fn limits_readiness_and_bind_sources_are_checked() {
    assert_refused(&[
        (
            "no-limits",
            "services.load.deploy.resources.limits.cpus",
            2,
            "required",
        ),
        (
            "no-memory",
            "services.load.deploy.resources.limits.memory",
            6,
            "required",
        ),
        (
            "healthy-without-healthcheck",
            "services.load.depends_on.api",
            9,
            "no `healthcheck`",
        ),
        (
            "absolute-source",
            "services.load.volumes[1]",
            6,
            "`/srv/data` is a host path",
        ),
        (
            "substitution",
            "services.load.volumes[0]",
            4,
            "`$` substitution",
        ),
        (
            "stand-target",
            "services.load.volumes[0]",
            4,
            "under /stand",
        ),
        (
            "harness-outside",
            "services.load.volumes[0]",
            4,
            "leaves the folder",
        ),
    ]);
}

#[test]
fn a_harness_through_a_link_leaves_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    let root = Utf8Path::from_path(dir.path()).unwrap();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink("..", root.join("data"));
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir("..", root.join("data"));
    // Windows makes a link only in developer mode or for an administrator.
    if linked.is_err() {
        return;
    }
    let file = root.join("compose.yaml");
    let text = fs::read_to_string(fixture("harness-outside"))
        .unwrap()
        .replace("./scripts/../../tools", "./data/tools");
    fs::write(&file, text).unwrap();
    let Err(SpecError::Invalid {
        line, key, message, ..
    }) = load(&file, &Policy::new(None).unwrap())
    else {
        panic!("a bind through a link is not refused");
    };
    assert_eq!((key.as_str(), line), ("services.load.volumes[0]", 4));
    assert!(message.contains("through the link `data`"), "{message}");
}

#[test]
fn stand_repo_mounts_whole_on_w_app_without_a_volume() {
    assert_refused(&[
        ("tree-target", "services.load.volumes[0]", 4, "not `/app`"),
        (
            "tree-subfolder",
            "services.load.volumes[0]",
            4,
            "mounts whole",
        ),
        (
            "tree-read-only",
            "services.load.volumes[0]",
            4,
            "not read-only",
        ),
        ("harness-under-w", "services.load.volumes[0]", 4, "under /w"),
        ("volume-under-w", "services.load.volumes[0]", 4, "under /w"),
        (
            "tree-with-volume",
            "services.load.volumes[1]",
            6,
            "cannot add a disk",
        ),
        (
            "two-prepares",
            "services.install",
            2,
            "does not wait for `compile`",
        ),
    ]);
    // `migrate` with `${STAND_REPO}` after `prepare` is in the example.
    assert!(stand("no-prepare").prepare.is_none());
}

#[test]
fn every_service_on_the_tree_has_the_image_of_the_prepare_service_and_it_binds_nothing_else() {
    assert_refused(&[
        (
            "prepare-image",
            "services.api.image",
            7,
            "`node:24` is not `node:22` of the prepare service `prepare`",
        ),
        (
            "prepare-harness",
            "services.prepare.volumes[1]",
            6,
            "binds only `${STAND_REPO}`",
        ),
    ]);
    // Without a prepare service every one gets a fresh version disk, from any image.
    let stand = stand("no-prepare");
    assert_eq!(
        (
            service(&stand, "api").image.as_str(),
            service(&stand, "load").image.as_str()
        ),
        ("node:22", "node:24")
    );
}

#[test]
fn a_volume_is_the_disk_of_one_service_with_one_replica() {
    assert_refused(&[
        (
            "shared-volume",
            "services.replica.volumes[0]",
            8,
            "mounted by `db` too",
        ),
        (
            "volume-twice",
            "services.db.volumes[1]",
            4,
            "`data` is mounted twice",
        ),
        (
            "volume-replicas",
            "services.db.deploy.replicas",
            7,
            "one replica",
        ),
    ]);
}

#[test]
fn a_service_name_has_no_dot_and_is_not_reserved() {
    assert_refused(&[
        ("name-dot", "services.api.v2", 2, "has a `.`"),
        ("name-stand", "services.stand", 2, "reserved"),
        ("name-volume", "services.volume", 2, "reserved"),
    ]);
}

#[test]
fn the_graph_has_known_names_one_condition_per_service_and_no_cycle() {
    assert_refused(&[
        (
            "cycle",
            "services.a.depends_on",
            4,
            "`a`, `b` and `c` wait for each other",
        ),
        (
            "unknown-dependency",
            "services.load.depends_on[0]",
            5,
            "no service `db`",
        ),
        (
            "workload-depended",
            "services.api.depends_on[0]",
            4,
            "the workload `load`",
        ),
        (
            "conditions-differ",
            "services.load.depends_on.seed",
            14,
            "`load` waits for `seed` until service_healthy, and `api` until \
             service_completed_successfully",
        ),
    ]);
}

#[test]
fn a_probe_runs_in_a_long_running_service_and_after_setup_in_one_setup_starts() {
    let stand = stand("probes");
    let probes: Vec<String> = stand
        .probes
        .iter()
        .map(|(name, probe)| {
            let ignore: Vec<&str> = probe.ignore.iter().map(|path| path.as_str()).collect();
            format!(
                "{} in {} {:?} key {:?} ignore {ignore:?}",
                name.as_str(),
                probe.service.as_str(),
                probe.at,
                probe.key.as_ref().map(|key| key.as_str()),
            )
        })
        .collect();
    assert_eq!(
        probes,
        [
            "logs in api OnFailure key None ignore []",
            r#"rows in db AfterSetup key Some("") ignore ["/a~1b", "/items/0/sku"]"#,
        ]
    );
    assert_refused(&[
        (
            "probe-in-workload",
            "x-stand.probes.rows.in",
            23,
            "is the workload",
        ),
        (
            "probe-in-setup",
            "x-stand.probes.rows.in",
            23,
            "is a setup service",
        ),
        (
            "probe-after-setup",
            "x-stand.probes.rows.at",
            24,
            "does not run in setup",
        ),
        (
            "probe-empty-run",
            "x-stand.probes.rows.run",
            25,
            "an empty argv",
        ),
        (
            "probe-bad-pointer",
            "x-stand.probes.rows.key",
            26,
            "not a JSON pointer",
        ),
    ]);
}

#[test]
fn expect_reads_changes_without_quotes_and_holds_them_to_the_policy() {
    let stand = stand("expect-changes");
    let costs: Vec<(&str, Change)> = stand
        .expect
        .iter()
        .map(|cost| (cost.measure.as_str(), cost.change))
        .collect();
    assert_eq!(costs[0], ("stand.wall", Change::None));
    assert!(matches!(costs[1], ("api.cpu", Change::UpTo(percent)) if percent.get() == 10));
    assert!(matches!(costs[2], ("api.memory", Change::DownTo(percent)) if percent.get() == 10));
    assert_eq!(
        costs[3..],
        [
            ("api.shutdown", Change::Down),
            ("api:3000.connections", Change::Unknown)
        ]
    );
    let unchanged = &stand.second_pass.as_ref().unwrap().unchanged;
    let unchanged: Vec<(&str, &str)> = unchanged
        .iter()
        .map(|file| (file.file.as_str(), file.reason.as_str()))
        .collect();
    assert_eq!(
        unchanged,
        [
            ("probe.orders.jsonl", "a second pass sees the same orders"),
            ("results/orders.jsonl", "the workload reads the same orders")
        ]
    );

    assert_refused(&[
        (
            "cost-wording",
            "x-stand.expect.cost[0].change",
            9,
            "`up to +10%`: no sign",
        ),
        (
            "cost-no-sign",
            "x-stand.expect.cost[0].change",
            9,
            "`10%`: no sign",
        ),
        (
            "cost-no-percent",
            "x-stand.expect.cost[0].change",
            9,
            "`+10`: no `%`",
        ),
        (
            "cost-below-5",
            "x-stand.expect.cost[0].change",
            9,
            "X is below 5",
        ),
        (
            "cost-wide",
            "x-stand.expect.cost[0].change",
            9,
            "wider than `max_tolerance`",
        ),
        (
            "cost-fall-over-100",
            "x-stand.expect.cost[0].change",
            9,
            "X is above 100",
        ),
        (
            "cost-empty-reason",
            "x-stand.expect.cost[0].reason",
            9,
            "empty",
        ),
        (
            "unchanged-without-second-pass",
            "x-stand.expect.unchanged",
            8,
            "`x-stand.second_pass: true`",
        ),
    ]);
    // A wider policy takes what the default one refuses, but no policy a fall past 100%.
    load(&fixture("cost-wide"), &Policy::new(Some(50)).unwrap()).unwrap();
    let widest = Policy::new(Some(1000)).unwrap();
    let Err(SpecError::Invalid { message, .. }) = load(&fixture("cost-fall-over-100"), &widest)
    else {
        panic!("a fall past 100% is not refused");
    };
    assert!(message.contains("X is above 100"), "{message}");
}

#[test]
fn change_and_policy_read_the_grammar_of_expect_and_accept() {
    let percent = |text: &str| match Change::parse(text) {
        Ok(Change::UpTo(percent)) => format!("up to {}", percent.get()),
        Ok(Change::DownTo(percent)) => format!("down to {}", percent.get()),
        other => format!("{other:?}"),
    };
    assert_eq!(
        ["+5%", "+1000%", "-100%"].map(percent),
        ["up to 5", "up to 1000", "down to 100"]
    );
    for text in [
        "up to +10%",
        "10%",
        "+10",
        "+4%",
        "+10.5%",
        "+1001%",
        "-101%",
        "None",
        "",
    ] {
        let error = Change::parse(text).unwrap_err();
        assert!(error.to_string().ends_with("with X from 5"), "{error}");
    }

    let policy = Policy::new(None).unwrap();
    assert_eq!(policy.max_tolerance.get(), 25);
    let within = |text: &str| Change::parse(text).unwrap().within(&policy);
    assert_eq!(
        ["+25%", "-25%", "+26%", "-26%", "none", "down", "unknown"].map(within),
        [true, true, false, false, true, true, true]
    );
    assert_eq!(Policy::new(Some(1000)).unwrap().max_tolerance.get(), 1000);
    assert!(matches!(Policy::new(Some(0)), Err(PolicyError(0))));
    assert!(matches!(Policy::new(Some(1001)), Err(PolicyError(1001))));
}

#[test]
fn a_fault_targets_an_exposed_port_and_slow_has_a_delay() {
    let down = stand("fault-down").fault.unwrap();
    assert_eq!(
        (
            down.target.service.as_str(),
            down.target.port.get(),
            down.mode
        ),
        ("db", 5432, FaultMode::Down)
    );
    let slow = stand("fault-slow").fault.unwrap();
    assert_eq!(slow.mode, FaultMode::Slow(Duration::from_secs(2)));
    assert_refused(&[
        (
            "fault-unexposed",
            "x-stand.fault.target",
            12,
            "does not expose `6379`",
        ),
        (
            "fault-slow-without-delay",
            "x-stand.fault.delay",
            12,
            "required",
        ),
        (
            "fault-unknown-mode",
            "x-stand.fault.mode",
            12,
            "`flaky` is not",
        ),
        (
            "fault-down-delay",
            "x-stand.fault.delay",
            12,
            "for `mode: slow`",
        ),
    ]);
}

#[test]
fn what_the_stand_would_not_honour_is_refused_at_its_key() {
    assert_refused(&[
        (
            "mount-twice",
            "services.load.volumes[1]",
            4,
            "a second mount on `/harness`",
        ),
        ("port-twice", "services.load.expose[1]", 4, "listed twice"),
        ("root-target", "services.load.volumes[0]", 4, "on `/`"),
        (
            "cost-twice",
            "x-stand.expect.cost[1].measure",
            10,
            "`stand.wall` is declared twice",
        ),
        (
            "unchanged-twice",
            "x-stand.expect.unchanged[1].file",
            11,
            "`orders.jsonl` is named twice",
        ),
        (
            "tag",
            "services.load.image",
            3,
            "YAML tags are not supported",
        ),
        ("volume-keys", "volumes.data.driver", 7, "takes no key"),
    ]);
}

#[test]
fn the_recipe_has_every_file_of_the_folder_and_changed_names_what_differs() {
    let example = stand("example").recipe;
    let files: Vec<&str> = example.files.keys().map(String::as_str).collect();
    assert_eq!(files, ["compose.yaml", "load.js", "ping.js"]);
    let sha256 = |text: &str| text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit());
    assert!(example.files.values().all(|sha| sha256(sha)) && sha256(&example.digest));

    let sealed = stand("recipe-sealed").recipe;
    let edited = stand("recipe-edited").recipe;
    assert!(sealed.files.contains_key("lib/util.js"));
    assert_eq!(edited.changed(&sealed), ["load.js", "new.js", "old.js"]);
    assert_ne!(edited.digest, sealed.digest);
    let same = stand("recipe-same").recipe;
    assert!(same.changed(&sealed).is_empty());
    assert_eq!(same.digest, sealed.digest);
}

#[test]
fn the_digest_does_not_depend_on_the_order_files_were_written_in() {
    let compose = fs::read_to_string(fixture("recipe-sealed")).unwrap();
    let files = [
        ("compose.yaml", compose.as_str()),
        ("a/z.js", "z"),
        ("b.js", "b"),
        ("a/c/d.js", "d"),
    ];
    let mut reversed = files;
    reversed.reverse();
    let forward = written(&files);
    assert_eq!(forward.files.len(), 4);
    assert_eq!(forward, written(&reversed));
}

/// The recipe of a temporary folder with the files written in this order.
fn written(files: &[(&str, &str)]) -> Recipe {
    let dir = tempfile::tempdir().unwrap();
    let root = Utf8Path::from_path(dir.path()).unwrap();
    for (path, text) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    stand_at(&root.join("compose.yaml")).recipe
}

#[test]
fn an_error_names_the_file_line_and_key() {
    let error = load_case("unknown-key").unwrap_err().to_string();
    assert!(
        error.ends_with(
            "compose.yaml:9: services.load.deploy.resources.limits.pids: unknown key `pids`: \
             here the stand takes `cpus`, `memory`"
        ),
        "{error}"
    );
}

#[test]
fn lines_and_numbers_hold_with_crlf_and_a_bom() {
    let lf = |case: &str| {
        fs::read_to_string(fixture(case))
            .unwrap()
            .replace("\r\n", "\n")
    };
    let forms = |text: String| {
        let crlf = text.replace('\n', "\r\n");
        [format!("\u{feff}{crlf}"), crlf, text]
    };
    for text in forms(lf("unknown-key")) {
        let Err(SpecError::Invalid { line, key, .. }) = load_text(&text) else {
            panic!("{text:?} is not refused as invalid");
        };
        assert_eq!(
            (key.as_str(), line),
            ("services.load.deploy.resources.limits.pids", 9)
        );
    }
    for text in forms(lf("values")) {
        let stand = load_text(&text).unwrap();
        let environment = &service(&stand, "worker").environment;
        assert_eq!(
            (environment["RATIO"].as_str(), environment["PORT"].as_str()),
            ("0.50", "+3000")
        );
    }
}

/// The stand file of a temporary folder with `text`.
fn load_text(text: &str) -> Result<Stand, SpecError> {
    let dir = tempfile::tempdir().unwrap();
    let file = Utf8Path::from_path(dir.path())
        .unwrap()
        .join("compose.yaml");
    fs::write(&file, text).unwrap();
    load(&file, &Policy::new(None).unwrap())
}

#[test]
fn a_file_that_is_not_there_is_missing() {
    let Err(SpecError::Missing(file)) = load_case("no-such-case") else {
        panic!("a file that is not there is not missing");
    };
    assert!(file.ends_with("no-such-case/compose.yaml"), "{file}");
}
