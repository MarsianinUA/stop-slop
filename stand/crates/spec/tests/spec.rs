use std::collections::BTreeMap;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use spec::{
    Condition, Demand, Mount, Outbound, Profiler, Service, ServiceKind, ServiceName, SpecError,
    Stand, load,
};

/// `tests/fixtures/<case>/compose.yaml`. Every refusal has its own folder: a small file that is
/// valid but for the one thing the case is named after.
fn fixture(case: &str) -> Utf8PathBuf {
    Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(case)
        .join("compose.yaml")
}

fn stand(case: &str) -> Stand {
    load(&fixture(case)).unwrap_or_else(|error| panic!("{case}: {error}"))
}

fn service<'a>(stand: &'a Stand, name: &str) -> &'a Service {
    stand
        .services
        .iter()
        .find(|(known, _)| known.as_str() == name)
        .map(|(_, service)| service)
        .unwrap_or_else(|| panic!("no service {name}"))
}

/// Each case is refused at its key and line, with a message that has the words.
fn assert_refused(cases: &[(&str, &str, u32, &str)]) {
    for &(case, key, line, words) in cases {
        let Err(SpecError::Invalid {
            file,
            line: refused_line,
            key: refused_key,
            message,
        }) = load(&fixture(case))
        else {
            panic!("{case} is not refused as invalid");
        };
        assert!(
            file.ends_with(format!("{case}/compose.yaml")),
            "{case}: {file}"
        );
        assert_eq!(
            (refused_key.as_str(), refused_line),
            (key, line),
            "{case}: {message}"
        );
        assert!(message.contains(words), "{case}: {message}");
    }
}

fn names(names: &[ServiceName]) -> Vec<&str> {
    names.iter().map(ServiceName::as_str).collect()
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
    let levels: Vec<Vec<&str>> = stand.levels.iter().map(|level| names(level)).collect();
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
    assert_eq!(
        db.expose.iter().map(|port| port.get()).collect::<Vec<_>>(),
        [5432]
    );
    let health = db.healthcheck.as_ref().unwrap();
    assert_eq!(health.test, ["pg_isready", "-U", "postgres"]);
    assert_eq!(
        (
            health.interval,
            health.timeout,
            health.retries,
            health.start_period
        ),
        (
            Duration::from_secs(1),
            Duration::from_secs(30),
            30,
            Duration::ZERO
        )
    );
    assert_eq!((db.cpus.get(), db.memory.get()), (1000, 1 << 30));

    let migrate = service(&stand, "migrate");
    let waits: Vec<(&str, Condition)> = migrate
        .depends_on
        .iter()
        .map(|(name, condition)| (name.as_str(), *condition))
        .collect();
    assert_eq!(
        waits,
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
            (service.stop_signal.as_str(), service.stop_grace),
            ("SIGTERM", Duration::from_secs(10))
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
    assert_eq!(
        cache
            .expose
            .iter()
            .map(|port| port.get())
            .collect::<Vec<_>>(),
        [6379]
    );
    let health = cache.healthcheck.as_ref().unwrap();
    assert_eq!(health.test, ["sh", "-c", "redis-cli ping"]);
    assert_eq!(
        (
            health.interval,
            health.timeout,
            health.retries,
            health.start_period
        ),
        (
            Duration::from_secs(30),
            Duration::from_secs(30),
            3,
            Duration::ZERO
        )
    );
    assert_eq!(
        (cache.stop_signal.as_str(), cache.stop_grace),
        ("SIGINT", Duration::from_secs(90))
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
    // A boolean reads as Compose writes it, a number as YAML gives it back, `$$` as `$`.
    assert_eq!(
        worker.environment,
        BTreeMap::from([
            ("DEBUG".to_owned(), "true".to_owned()),
            ("PRICE".to_owned(), "$5".to_owned()),
            ("RATIO".to_owned(), "0.5".to_owned()),
        ])
    );
    let health = worker.healthcheck.as_ref().unwrap();
    assert_eq!(health.test, ["sh", "-c", "test -f /tmp/ready"]);
    assert_eq!(
        (
            health.interval,
            health.timeout,
            health.retries,
            health.start_period
        ),
        (
            Duration::from_millis(500),
            Duration::from_secs(2),
            5,
            Duration::from_secs(60)
        )
    );
    assert_eq!((worker.cpus.get(), worker.memory.get()), (500, 1 << 30));

    let load = service(&stand, "load");
    let waits: Vec<(&str, Condition)> = load
        .depends_on
        .iter()
        .map(|(name, condition)| (name.as_str(), *condition))
        .collect();
    assert_eq!(
        waits,
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
            "services.load.depends_on.db",
            4,
            "no service `db`",
        ),
        (
            "workload-depended",
            "services.api.depends_on.load",
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
fn an_error_names_the_file_line_and_key() {
    let error = load(&fixture("unknown-key")).unwrap_err().to_string();
    assert!(
        error.ends_with(
            "compose.yaml:9: services.load.deploy.resources.limits.pids: unknown key `pids`: \
             here the stand takes `cpus`, `memory`"
        ),
        "{error}"
    );
}

#[test]
fn a_file_that_is_not_there_is_missing() {
    let Err(SpecError::Missing(file)) = load(&fixture("no-such-case")) else {
        panic!("a file that is not there is not missing");
    };
    assert!(file.ends_with("no-such-case/compose.yaml"), "{file}");
}
