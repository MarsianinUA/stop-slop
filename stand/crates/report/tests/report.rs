//! `read_outputs` and `make` on values of `Raw` and `Context` (blueprint, Tests `report.rs`). No
//! VM: a temporary folder stands in for the run folder that `run` leaves.

use std::collections::BTreeMap;
use std::fs;
use std::time::{Duration, SystemTime};

use camino::{Utf8Path, Utf8PathBuf};
use profile::{Called, Coverage, ModuleTime, Sampled, Samples};
use report::{
    BeforeInfo, BeforeOutcome, Cause, Context, CoverageOutcome, Failure, FaultOutcome, FaultRepeat,
    Flag, InstanceOutput, Intent, Measure, Phase, Previous, ProbeRan, ProfileOutcome, Raw,
    RepeatOutput, Report, Role, Setup, SetupFrom, SetupSide, Src, Verdict, make, read_outputs,
};
use sandbox::{ExecOutcome, Metrics, PortStats, Stopped, Unexposed};
use serde_json::json;
use tempfile::TempDir;

const GIB: u64 = 1 << 30;
const RUN: &str = "20261022-100000";
const PREVIOUS: &str = "20261022-094500";
const HEAD_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const BEFORE_TREEISH: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";
const BEFORE_TREE: &str = "7d1e09f3c2b4a5968778695a4b3c2d1e0f9a8b7c";
const DIGEST: &str = "9f2c41aa6b0e4d3c8a7f5e1d2c4b6a8f0e9d7c5b3a1f2e4d6c8b0a9f7e5d3c1b";

// --- helpers ---------------------------------------------------------------------------------

/// The run folder `run` leaves: `<name>/out/` per setup and repeat, one log per instance.
struct Folder(TempDir);

impl Folder {
    fn new() -> Folder {
        Folder(tempfile::tempdir().expect("a temporary folder"))
    }

    fn path(&self) -> &Utf8Path {
        Utf8Path::from_path(self.0.path()).expect("a UTF-8 temporary folder")
    }

    /// `<name>/out/` with `files` in it.
    fn out(&self, name: &str, files: &[(&str, &str)]) -> Utf8PathBuf {
        let out = self.path().join(name).join("out");
        fs::create_dir_all(&out).expect("create out");
        for (file, text) in files {
            fs::write(out.join(file), text).expect("write an output");
        }
        out
    }

    /// `<name>.log`: `cut` is the measured window, `after` the lines past the cut.
    fn log(&self, name: &str, cut: &str, after: &str) -> (Utf8PathBuf, u64) {
        let path = self.path().join(format!("{name}.log"));
        fs::write(&path, format!("{cut}{after}")).expect("write a log");
        (path, cut.len() as u64)
    }
}

fn metrics(cpu_ms: u64, memory: u64) -> Metrics {
    Metrics {
        cpu: Duration::from_millis(cpu_ms),
        memory_peak: memory,
        memory_limit: GIB,
        ports: Vec::new(),
        unexposed: Vec::new(),
    }
}

fn port(target: &str, connections: u64) -> PortStats {
    PortStats {
        target: target.to_owned(),
        connections,
        open: 0,
        bytes_out: 0,
        bytes_in: 0,
        http: None,
        drops: 0,
    }
}

fn instance(
    service: &str,
    role: Role,
    metrics: Metrics,
    (log, log_cut): (Utf8PathBuf, u64),
) -> InstanceOutput {
    let stopped = Stopped {
        after: Duration::from_millis(100),
        killed: false,
    };
    InstanceOutput {
        service: service.to_owned(),
        replica: 1,
        role,
        metrics,
        log,
        log_cut,
        stopped: (role == Role::LongRunning).then_some(stopped),
        profile: None,
    }
}

/// A measured repeat with the long-running `api` and the workload `load`, which calls `api:3000`.
fn plain(folder: &Folder, side: &str, repeat: u32, wall: u64) -> RepeatOutput {
    let dir = format!("repeat-{repeat}-{side}");
    let mut load = metrics(500, 50 << 20);
    load.ports.push(port("api:3000", 10));
    let api_log = folder.log(&format!("{dir}-api"), "ready\n", "");
    RepeatOutput {
        repeat,
        wall: Duration::from_millis(wall),
        instances: vec![
            instance("api", Role::LongRunning, metrics(1000, 100 << 20), api_log),
            instance(
                "load",
                Role::Workload,
                load,
                folder.log(&format!("{dir}-load"), "", ""),
            ),
        ],
        probes: Vec::new(),
        out_dir: folder.out(&dir, &[]),
    }
}

/// One repeat per wall time.
fn repeats(folder: &Folder, side: &str, walls: &[u64]) -> Vec<RepeatOutput> {
    let numbered = walls.iter().zip(1..);
    numbered
        .map(|(wall, repeat)| plain(folder, side, repeat, *wall))
        .collect()
}

fn setup_side(folder: &Folder, side: &str) -> SetupSide {
    SetupSide {
        sizes: BTreeMap::new(),
        probes: Vec::new(),
        out_dir: folder.out(&format!("setup-{side}"), &[]),
    }
}

fn raw(folder: &Folder, head: Vec<RepeatOutput>, before: Option<Vec<RepeatOutput>>) -> Raw {
    Raw {
        digest: DIGEST.to_owned(),
        repeat: u32::try_from(head.len()).expect("a few repeats"),
        setup_timeout: Duration::from_secs(600),
        timeout: Duration::from_secs(300),
        harness: vec!["/harness".to_owned()],
        probes: BTreeMap::new(),
        setup: Setup {
            head: setup_side(folder, "head"),
            before: before.as_ref().map(|_| setup_side(folder, "before")),
        },
        head,
        before: before.map(BeforeOutcome::Passed),
        coverage: None,
        fault: None,
        second_pass: None,
    }
}

fn at(time: &str) -> SystemTime {
    humantime::parse_rfc3339(time).expect("an RFC 3339 time")
}

fn context<'r>(raw: &Raw, previous: Previous<'r>, pool: &'r [Report]) -> Context<'r> {
    let before = raw.before.as_ref().map(|_| BeforeInfo {
        intent: Intent::Session,
        treeish: BEFORE_TREEISH.to_owned(),
        tree: BEFORE_TREE.to_owned(),
        taken_at: at("2026-10-22T09:12:00Z"),
    });
    Context {
        run: RUN.to_owned(),
        created_at: at("2026-10-22T10:00:00Z"),
        root: "E:/work/app".into(),
        head_tree: HEAD_TREE.to_owned(),
        before,
        previous,
        pool,
        setup_from: SetupFrom::default(),
    }
}

fn report_of(raw: &Raw, context: &Context<'_>, pool: &[Utf8PathBuf]) -> Report {
    make(
        raw,
        &read_outputs(raw, pool).expect("the outputs read"),
        context,
    )
}

fn report(raw: &Raw) -> Report {
    report_of(raw, &context(raw, Previous::None, &[]), &[])
}

fn measure<'r>(report: &'r Report, name: &str) -> &'r Measure {
    let found = report.measures.iter().find(|measure| measure.name == name);
    found.unwrap_or_else(|| panic!("no measure {name} in {:?}", names(report)))
}

fn names(report: &Report) -> Vec<&str> {
    let names = report.measures.iter();
    names.map(|measure| measure.name.as_str()).collect()
}

fn verdicts(measure: &Measure) -> (Option<Verdict>, Verdict) {
    let before = measure.before.as_ref();
    (
        before.map(|before| before.verdict),
        measure.previous.verdict,
    )
}

fn probe_ran(name: &str, outcome: ExecOutcome, out: &Utf8Path) -> ProbeRan {
    ProbeRan {
        name: name.to_owned(),
        outcome,
        out: out.join(format!("probe.{name}.jsonl")),
    }
}

fn read(functions: Vec<Sampled>) -> ProfileOutcome {
    let local_ms = functions.iter().map(|function| function.self_ms).sum();
    ProfileOutcome::Read(Samples {
        total_ms: local_ms,
        local_ms,
        unmapped_ms: 0.0,
        processes: 1,
        lost_processes: 0,
        functions,
        modules: Vec::new(),
    })
}

fn sampled(file: &str, name: &str, line: u32, self_ms: f64) -> Sampled {
    Sampled {
        file: file.into(),
        line,
        column: 21,
        name: name.to_owned(),
        self_ms,
        total_ms: self_ms * 2.0,
        lines: Vec::new(),
    }
}

fn killed() -> Option<Stopped> {
    Some(Stopped {
        after: Duration::from_secs(10),
        killed: true,
    })
}

// --- measures ----------------------------------------------------------------------------------

#[test]
fn measures_take_the_median_the_spread_and_the_verdicts_against_before_and_the_previous_run() {
    let folder = Folder::new();
    let earlier = raw(
        &folder,
        repeats(&folder, "previous", &[131, 132, 133]),
        None,
    );
    let previous = report(&earlier);
    let mut head = repeats(&folder, "head", &[130, 132, 134]);
    let mut before = repeats(&folder, "before", &[100, 101, 102]);
    for repeat in head.iter_mut().chain(&mut before) {
        repeat.instances[0].metrics.ports.push(port("db:5432", 3));
    }
    let raw = raw(&folder, head, Some(before));
    let report = report_of(&raw, &context(&raw, Previous::Read(&previous), &[]), &[]);

    assert_eq!(
        (report.setup_timeout.as_str(), report.timeout.as_str()),
        ("10m", "5m")
    );
    assert_eq!(report.created_at, "2026-10-22T10:00:00Z");
    let wall = measure(&report, "stand.wall");
    assert_eq!(wall.samples, [130.0, 132.0, 134.0]);
    assert_eq!((wall.median, wall.spread), (132.0, 0.0303));
    let against = wall.before.as_ref().expect("a before");
    assert_eq!(against.samples, [100.0, 101.0, 102.0]);
    assert_eq!(
        (against.median, against.verdict),
        (Some(101.0), Verdict::Regressed)
    );
    assert_eq!(
        (wall.previous.median, wall.previous.verdict),
        (Some(132.0), Verdict::WithinNoise)
    );
    assert_eq!(
        verdicts(measure(&report, "api.cpu")),
        (Some(Verdict::WithinNoise), Verdict::WithinNoise)
    );
    // The previous run never called `db:5432`.
    let db = measure(&report, "db:5432.connections");
    assert_eq!(
        (db.previous.median, db.previous.verdict),
        (None, Verdict::NoBaseline)
    );
    assert_eq!(report.previous_run.as_deref(), Some(RUN));
    assert!(measure(&previous, "stand.wall").before.is_none());
}

#[test]
fn error_lines_and_the_5xx_share_rising_from_zero_regress() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[100, 100, 100]);
    let mut before = repeats(&folder, "before", &[100, 100, 100]);
    for (index, repeat) in head.iter_mut().enumerate() {
        let (log, cut) = folder.log(&format!("head-api-{index}"), "ready\nERROR: boom\n", "");
        repeat.instances[0].log = log;
        repeat.instances[0].log_cut = cut;
        repeat.instances[1].metrics.ports[0].http = Some([0, 99, 0, 0, 1]);
    }
    for repeat in &mut before {
        repeat.instances[1].metrics.ports[0].http = Some([0, 100, 0, 0, 0]);
    }
    let report = report(&raw(&folder, head, Some(before)));

    let errors = measure(&report, "api.error-lines");
    assert_eq!(errors.samples, [1.0, 1.0, 1.0]);
    assert_eq!(verdicts(errors).0, Some(Verdict::Regressed));
    let share = measure(&report, "api:3000.http-5xx-share");
    assert_eq!(share.samples, [0.01, 0.01, 0.01]);
    assert_eq!(verdicts(share).0, Some(Verdict::Regressed));
}

#[test]
fn every_measure_is_subject_dot_metric_and_the_stand_measured_it() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[100]);
    head[0].instances[0].metrics.ports.push(port("db:5432", 3));
    head[0].instances[1].metrics.ports[0].http = Some([0, 10, 0, 0, 0]);
    let mut raw = raw(&folder, head, None);
    raw.setup.head.sizes.insert("pgdata".to_owned(), 4096);
    let report = report(&raw);

    assert_eq!(
        names(&report),
        [
            "stand.wall",
            "api.cpu",
            "api.memory",
            "api.error-lines",
            "api.shutdown",
            "api:3000.connections",
            "api:3000.bytes",
            "api:3000.http-5xx-share",
            "db:5432.connections",
            "db:5432.bytes",
            "volume:pgdata.bytes",
        ]
    );
    for measure in &report.measures {
        let (subject, metric) = measure.name.rsplit_once('.').expect("<subject>.<metric>");
        assert!(
            !subject.is_empty() && !metric.is_empty(),
            "{}",
            measure.name
        );
        let kebab = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-';
        assert!(metric.bytes().all(kebab), "{}", measure.name);
        assert_eq!(measure.src, Src::Stand, "{}", measure.name);
    }
}

#[test]
fn a_neutral_measure_only_changes_and_a_lower_one_improves_or_regresses() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[100, 100, 100]);
    for repeat in &mut head {
        repeat.instances[1].metrics.ports[0].connections = 20;
    }
    let before = repeats(&folder, "before", &[200, 200, 200]);
    let mut raw = raw(&folder, head, Some(before));
    raw.setup.head.sizes.insert("pgdata".to_owned(), 500);
    let before_setup = raw.setup.before.as_mut().expect("a before setup");
    before_setup.sizes.insert("pgdata".to_owned(), 1000);
    let report = report(&raw);

    assert_eq!(
        verdicts(measure(&report, "stand.wall")).0,
        Some(Verdict::Improved)
    );
    // More connections and less data both only change: neither side is better.
    assert_eq!(
        verdicts(measure(&report, "api:3000.connections")).0,
        Some(Verdict::Changed)
    );
    assert_eq!(
        verdicts(measure(&report, "volume:pgdata.bytes")).0,
        Some(Verdict::Changed)
    );
}

#[test]
fn error_lines_and_node_warnings_count_before_the_cut_and_the_slowest_stop_is_the_shutdown() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[100]);
    let cut = "ready\n(node:7) MaxListenersExceededWarning: Possible EventEmitter memory leak\nERROR: boom\n";
    let (log, log_cut) = folder.log("api-1", cut, "ERROR: past the cut\n");
    let first = &mut head[0].instances[0];
    first.log = log;
    first.log_cut = log_cut;
    first.stopped = Some(Stopped {
        after: Duration::from_millis(250),
        killed: false,
    });
    let second_log = folder.log("api-2", "[ERROR] x\n", "");
    let mut second = instance(
        "api",
        Role::LongRunning,
        metrics(1000, 100 << 20),
        second_log,
    );
    second.replica = 2;
    second.stopped = Some(Stopped {
        after: Duration::from_millis(400),
        killed: false,
    });
    head[0].instances.push(second);
    let report = report(&raw(&folder, head, None));

    assert_eq!(measure(&report, "api.error-lines").samples, [3.0]);
    assert_eq!(measure(&report, "api.shutdown").samples, [400.0]);
    assert_eq!(measure(&report, "api.cpu").samples, [2000.0]);
}

#[test]
fn a_failed_before_has_no_baseline_and_its_failure_is_in_the_report() {
    let folder = Folder::new();
    let failure = Failure {
        phase: Phase::Setup,
        service: Some("prepare".to_owned()),
        repeat: None,
        cause: Cause::Exit(1),
        logs: Some(folder.path().join("setup-before")),
        probes: Vec::new(),
    };
    let mut raw = raw(&folder, repeats(&folder, "head", &[100, 100, 100]), None);
    raw.setup.before = Some(setup_side(&folder, "before"));
    raw.before = Some(BeforeOutcome::Failed(failure.clone()));
    let report = report(&raw);

    let before = report.before.as_ref().expect("a before");
    assert_eq!(before.failed.as_ref(), Some(&failure));
    let wall = measure(&report, "stand.wall");
    let wall = wall.before.as_ref().expect("a verdict against before");
    assert_eq!((wall.median, wall.verdict), (None, Verdict::NoBaseline));
    assert!(wall.samples.is_empty());
}

#[test]
fn setup_cached_names_the_run_that_built_the_setup_of_each_version() {
    let folder = Folder::new();
    let before = Some(repeats(&folder, "before", &[100]));
    let raw = raw(&folder, repeats(&folder, "head", &[100]), before);
    let mut context = context(&raw, Previous::None, &[]);
    context.setup_from = SetupFrom {
        head: None,
        before: Some(PREVIOUS.to_owned()),
    };
    let report = report_of(&raw, &context, &[]);

    assert_eq!(report.head.setup.cached, None);
    let before = report.before.as_ref().expect("a before");
    assert_eq!(before.setup.cached.as_deref(), Some(PREVIOUS));
}

#[test]
fn pooled_runs_join_their_samples_and_keep_the_spread_of_one_attempt() {
    let folder = Folder::new();
    let pool: Vec<Report> = ["a", "b"]
        .iter()
        .map(|name| {
            report(&raw(
                &folder,
                repeats(&folder, name, &[100, 110, 120]),
                None,
            ))
        })
        .collect();
    let raw = raw(&folder, repeats(&folder, "head", &[200, 220, 240]), None);
    let report = report_of(&raw, &context(&raw, Previous::None, &pool), &[]);

    let wall = measure(&report, "stand.wall");
    assert_eq!(wall.samples, [200.0, 220.0, 240.0]);
    assert_eq!(wall.median, 120.0);
    // Each attempt spreads by 20/110 = 40/220; three of them spread no wider than one.
    assert_eq!(wall.spread, 0.1818);
}

#[test]
fn volume_sizes_of_one_setup_origin_count_once_in_the_pool() {
    let folder = Folder::new();
    let sized = |name: &str, bytes: u64, origin: &str| {
        let mut raw = raw(&folder, repeats(&folder, name, &[100]), None);
        raw.setup.head.sizes.insert("pgdata".to_owned(), bytes);
        (raw, origin.to_owned())
    };
    let pooled = |runs: &[(Raw, String)]| -> Vec<Report> {
        let built = runs.iter().zip(["20261022-090000", "20261022-091000"]);
        let built = built.map(|((raw, origin), run)| {
            let mut context = context(raw, Previous::None, &[]);
            context.run = run.to_owned();
            context.setup_from.head = Some(origin.clone());
            report_of(raw, &context, &[])
        });
        built.collect()
    };
    let (this, _) = sized("head", 400, "x");
    let context_x = |pool: &[Report]| {
        let mut context = context(&this, Previous::None, pool);
        context.setup_from.head = Some("x".to_owned());
        measure(&report_of(&this, &context, &[]), "volume:pgdata.bytes").median
    };

    let one = pooled(&[sized("a", 100, "x"), sized("b", 200, "x")]);
    assert_eq!(context_x(&one), 100.0);
    let two = pooled(&[sized("a", 100, "x"), sized("b", 200, "y")]);
    assert_eq!(context_x(&two), 150.0);
}

// --- flags -------------------------------------------------------------------------------------

#[test]
fn each_fact_of_the_raw_outputs_gives_its_flag() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[100]);
    let repeat = &mut head[0];
    let out = repeat.out_dir.clone();
    let api = &mut repeat.instances[0];
    api.profile = Some(ProfileOutcome::NoProfile);
    api.stopped = killed();
    api.metrics.unexposed.push(Unexposed {
        name: "cache".to_owned(),
        attempts: 4,
    });
    let worker_log = folder.log("worker", "", "");
    let mut worker = instance(
        "worker",
        Role::LongRunning,
        metrics(10, 10 << 20),
        worker_log,
    );
    // 30 of the 100 ms of TypeScript found no map; the 300 ms of JavaScript are not TypeScript.
    worker.profile = Some(ProfileOutcome::Read(Samples {
        total_ms: 400.0,
        local_ms: 400.0,
        unmapped_ms: 30.0,
        processes: 3,
        lost_processes: 2,
        functions: vec![
            sampled("src/worker.ts", "work", 3, 50.0),
            sampled("scripts/seed.js", "seed", 1, 300.0),
        ],
        modules: vec![ModuleTime {
            file: "src/main.ts".into(),
            self_ms: 20.0,
        }],
    }));
    repeat.instances.push(worker);
    let load = &mut repeat.instances[1].metrics;
    load.memory_peak = GIB / 20 * 19;
    load.ports[0].connections = 300;
    load.ports[0].drops = 5;
    let orders = probe_ran("orders", ExecOutcome::Exited(2), &out);
    repeat.probes.push(orders);
    let mut raw = raw(&folder, head, None);
    let setup_out = raw.setup.head.out_dir.clone();
    let seed = probe_ran("seed", ExecOutcome::TimedOut, &setup_out);
    raw.setup.head.probes.push(seed);
    raw.coverage = Some(CoverageOutcome::Failed(Failure {
        phase: Phase::Ready,
        service: Some("api".to_owned()),
        repeat: None,
        cause: Cause::Unhealthy,
        logs: None,
        probes: Vec::new(),
    }));
    let report = report(&raw);

    let expected = [
        Flag::LowRepeats { samples: 1 },
        Flag::NoProfile {
            service: "api".to_owned(),
        },
        Flag::LostProcesses {
            service: "worker".to_owned(),
            count: 2,
        },
        Flag::UnmappedTime {
            service: "worker".to_owned(),
            share: 0.3,
        },
        Flag::MemoryNearLimit {
            service: "load".to_owned(),
            peak: GIB / 20 * 19,
            limit: GIB,
        },
        Flag::NetworkLimitHit {
            repeat: 1,
            target: "api:3000".to_owned(),
            drops: 5,
        },
        Flag::MissingExpose {
            name: "cache".to_owned(),
            attempts: 4,
        },
        Flag::CoverageUnknown {
            stage: Phase::Ready,
        },
        Flag::ProbeFailed {
            probe: "seed".to_owned(),
            side: "head".to_owned(),
            repeat: None,
            code: None,
        },
        Flag::ProbeFailed {
            probe: "orders".to_owned(),
            side: "head".to_owned(),
            repeat: Some(1),
            code: Some(2),
        },
        Flag::KilledAtStop {
            service: "api".to_owned(),
            side: "head".to_owned(),
            repeat: 1,
        },
    ];
    assert_eq!(report.flags, expected);
}

#[test]
fn network_limit_hit_takes_drops_and_256_connections_to_the_target_over_all_guests() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[100, 100, 100]);
    let mut before = repeats(&folder, "before", &[100, 100, 100]);
    // 200 from the workload and 56 from `api` itself reach `api:3000` in the first repeat.
    head[0].instances[1].metrics.ports[0].connections = 200;
    let mut back = port("api:3000", 56);
    back.drops = 1;
    head[0].instances[0].metrics.ports.push(back);
    // Drops under 256 connections are a target that was not ready: they add to no flag.
    let calls = [(255, 3), (300, 0)];
    for (repeat, (connections, drops)) in head[1..].iter_mut().zip(calls) {
        let port = &mut repeat.instances[1].metrics.ports[0];
        (port.connections, port.drops) = (connections, drops);
    }
    let calls = [(300, 2), (300, 4), (100, 5)];
    for (repeat, (connections, drops)) in before.iter_mut().zip(calls) {
        let port = &mut repeat.instances[1].metrics.ports[0];
        (port.connections, port.drops) = (connections, drops);
    }
    let report = report(&raw(&folder, head, Some(before)));

    let flags = report.flags.iter();
    let hits: Vec<&Flag> = flags
        .filter(|flag| matches!(flag, Flag::NetworkLimitHit { .. }))
        .collect();
    let hit = |repeat, drops| Flag::NetworkLimitHit {
        repeat,
        target: "api:3000".to_owned(),
        drops,
    };
    assert_eq!(hits, [&hit(1, 3), &hit(2, 4)]);
}

#[test]
fn a_target_with_two_replicas_hits_the_limit_from_512_connections() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[100, 100]);
    for (repeat, connections) in head.iter_mut().zip([511, 512]) {
        let log = folder.log(&format!("api-2-{}", repeat.repeat), "", "");
        let mut second = instance("api", Role::LongRunning, metrics(1000, 100 << 20), log);
        second.replica = 2;
        repeat.instances.push(second);
        let port = &mut repeat.instances[1].metrics.ports[0];
        (port.connections, port.drops) = (connections, 2);
    }
    let report = report(&raw(&folder, head, None));

    let flags = report.flags.iter();
    let hits: Vec<&Flag> = flags
        .filter(|flag| matches!(flag, Flag::NetworkLimitHit { .. }))
        .collect();
    let hit = Flag::NetworkLimitHit {
        repeat: 2,
        target: "api:3000".to_owned(),
        drops: 2,
    };
    assert_eq!(hits, [&hit]);
}

#[test]
fn a_run_without_a_profiled_service_is_unprofiled() {
    let folder = Folder::new();
    let plain = report(&raw(&folder, repeats(&folder, "a", &[100]), None));
    assert!(plain.flags.contains(&Flag::Unprofiled));
    let json = serde_json::to_value(&Flag::Unprofiled).unwrap();
    assert_eq!(json, json!({"kind": "unprofiled"}));
    assert_eq!(
        serde_json::from_value::<Flag>(json).unwrap(),
        Flag::Unprofiled
    );

    let mut head = repeats(&folder, "b", &[100]);
    head[0].instances[0].profile = Some(read(Vec::new()));
    let profiled = report(&raw(&folder, head, None));
    assert!(!profiled.flags.contains(&Flag::Unprofiled));
}

#[test]
fn previous_unreadable_and_recipe_changed_come_from_the_context() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[100]);
    head[0].instances[0].stopped = killed();
    let raw = raw(&folder, head, None);
    let unreadable = report_of(&raw, &context(&raw, Previous::Unreadable, &[]), &[]);
    // Found after the facts of `Raw`, listed in the order of the kinds.
    let expected = [
        Flag::LowRepeats { samples: 1 },
        Flag::Unprofiled,
        Flag::PreviousUnreadable,
        Flag::KilledAtStop {
            service: "api".to_owned(),
            side: "head".to_owned(),
            repeat: 1,
        },
    ];
    assert_eq!(unreadable.flags, expected);

    let mut other = raw_of(&folder, "other");
    other.digest = "0".repeat(64);
    let changed = report(&other);
    let flagged = report_of(&raw, &context(&raw, Previous::Read(&changed), &[]), &[]);
    assert!(flagged.flags.contains(&Flag::RecipeChanged));
    let same = report(&raw_of(&folder, "same"));
    let calm = report_of(&raw, &context(&raw, Previous::Read(&same), &[]), &[]);
    assert!(!calm.flags.contains(&Flag::RecipeChanged));
}

fn raw_of(folder: &Folder, name: &str) -> Raw {
    raw(folder, repeats(folder, name, &[100]), None)
}

#[test]
fn a_kill_at_the_stop_of_a_fault_repeat_is_no_flag() {
    let folder = Folder::new();
    let mut raw = raw(&folder, repeats(&folder, "head", &[100, 100, 100]), None);
    let held_log = folder.log("fault-db", "", "");
    let mut held = instance("db", Role::LongRunning, metrics(10, 10 << 20), held_log);
    held.stopped = killed();
    raw.fault = Some(FaultOutcome {
        target: "db:5432".to_owned(),
        mode: "down".to_owned(),
        head: vec![FaultRepeat {
            repeat: 1,
            workload: Some(0),
            exited: Vec::new(),
            failure: None,
            instances: vec![held],
        }],
        before: None,
    });
    let report = report(&raw);

    let mut flags = report.flags.iter();
    assert!(!flags.any(|flag| matches!(flag, Flag::KilledAtStop { .. })));
}

#[test]
fn a_wide_spread_is_high_noise_and_a_far_first_repeat_an_outlier() {
    let folder = Folder::new();
    let wall = || "stand.wall".to_owned();
    let noisy = report(&raw(&folder, repeats(&folder, "a", &[100, 125, 80]), None));
    assert!(noisy.flags.contains(&Flag::HighNoise { measure: wall() }));
    assert!(
        !noisy
            .flags
            .contains(&Flag::FirstRepeatOutlier { measure: wall() })
    );

    // The others lie within 1% of 100.5; the first is 19% above them, and the spread stays
    // under 20%.
    let warm = report(&raw(&folder, repeats(&folder, "b", &[120, 100, 101]), None));
    assert!(
        warm.flags
            .contains(&Flag::FirstRepeatOutlier { measure: wall() })
    );
    assert!(!warm.flags.contains(&Flag::HighNoise { measure: wall() }));
}

#[test]
fn low_repeats_counts_the_pooled_samples() {
    let folder = Folder::new();
    let alone = report(&raw(&folder, repeats(&folder, "a", &[100]), None));
    assert!(alone.flags.contains(&Flag::LowRepeats { samples: 1 }));

    let pool = vec![
        report(&raw(&folder, repeats(&folder, "b", &[100]), None)),
        report(&raw(&folder, repeats(&folder, "c", &[100]), None)),
    ];
    let raw = raw(&folder, repeats(&folder, "head", &[100]), None);
    let pooled = report_of(&raw, &context(&raw, Previous::None, &pool), &[]);
    let mut flags = pooled.flags.iter();
    assert!(!flags.any(|flag| matches!(flag, Flag::LowRepeats { .. })));
}

#[test]
fn a_failed_before_is_a_flag_with_its_stage() {
    let folder = Folder::new();
    let mut raw = raw(&folder, repeats(&folder, "head", &[100, 100, 100]), None);
    raw.setup.before = Some(setup_side(&folder, "before"));
    raw.before = Some(BeforeOutcome::Failed(Failure {
        phase: Phase::Workload,
        service: Some("load".to_owned()),
        repeat: Some(2),
        cause: Cause::Timeout,
        logs: None,
        probes: Vec::new(),
    }));
    let report = report(&raw);

    let flag = Flag::BeforeFailed {
        stage: Phase::Workload,
    };
    assert!(report.flags.contains(&flag));
    let json = serde_json::to_value(&flag).unwrap();
    assert_eq!(json, json!({"kind": "before-failed", "stage": "workload"}));
    assert_eq!(serde_json::from_value::<Flag>(json).unwrap(), flag);
}

// --- hot functions and coverage ----------------------------------------------------------------

/// Each repeat profiles `api` with these (file, name, line, column, self ms).
fn profiled(
    folder: &Folder,
    side: &str,
    functions: &[(&str, &str, u32, u32, f64)],
) -> Vec<RepeatOutput> {
    let mut repeats = repeats(folder, side, &[100, 100, 100]);
    for repeat in &mut repeats {
        let functions = functions.iter();
        let sampled = functions.map(|(file, name, line, column, self_ms)| Sampled {
            column: *column,
            ..sampled(file, name, *line, *self_ms)
        });
        repeat.instances[0].profile = Some(read(sampled.collect()));
    }
    repeats
}

fn called(file: &str, line: u32, column: u32, calls: u64) -> Called {
    Called {
        file: file.into(),
        line,
        column,
        name: String::new(),
        calls,
    }
}

#[test]
fn a_hot_function_keeps_its_key_when_its_lines_shift() {
    let folder = Folder::new();
    let build = |line, self_ms| [("src/orders.ts", "buildReport", line, 21, self_ms)];
    let earlier = raw(
        &folder,
        profiled(&folder, "previous", &build(41, 900.0)),
        None,
    );
    let previous = report(&earlier);
    let mut head = profiled(&folder, "head", &build(40, 800.0));
    // A second replica adds its time; `rare`, sampled in one repeat of three, has a median of 0.
    for repeat in &mut head {
        let log = folder.log(&format!("api-2-{}", repeat.repeat), "", "");
        let mut second = instance("api", Role::LongRunning, metrics(1000, 100 << 20), log);
        second.replica = 2;
        let mut functions = vec![sampled("src/orders.ts", "buildReport", 40, 100.0)];
        if repeat.repeat == 1 {
            functions.push(sampled("src/orders.ts", "rare", 90, 900.0));
        }
        second.profile = Some(read(functions));
        repeat.instances.push(second);
    }
    let before = profiled(&folder, "before", &build(38, 500.0));
    let raw = raw(&folder, head, Some(before));
    let report = report_of(&raw, &context(&raw, Previous::Read(&previous), &[]), &[]);

    assert_eq!(report.hot.len(), 1, "{:?}", report.hot);
    let hot = &report.hot[0];
    let key = (hot.service.as_str(), hot.file.as_str(), hot.name.as_str());
    assert_eq!(key, ("api", "src/orders.ts", "buildReport"));
    assert_eq!((hot.line, hot.column), (40, 21));
    assert_eq!((hot.self_ms, hot.total_ms), (900.0, 1800.0));
    let before = hot.before.as_ref().map(|before| before.verdict);
    assert_eq!(before, Some(Verdict::Regressed));
    assert_eq!(hot.previous.verdict, Verdict::WithinNoise);
}

#[test]
fn of_equal_names_the_nearest_line_wins_and_under_one_ms_is_left_out() {
    let folder = Folder::new();
    let head = profiled(
        &folder,
        "head",
        &[
            ("src/a.ts", "", 10, 21, 50.0),
            ("src/a.ts", "", 50, 21, 100.0),
            ("src/a.ts", "tiny", 70, 21, 0.5),
            ("src/b.ts", "", 30, 21, 100.0),
            ("src/c.ts", "", 5, 10, 100.0),
            ("src/d.ts", "", 8, 10, 100.0),
        ],
    );
    let before = profiled(
        &folder,
        "before",
        &[
            ("src/a.ts", "", 12, 21, 50.0),
            ("src/a.ts", "", 49, 21, 200.0),
            // Two lines either way: the earlier line wins.
            ("src/b.ts", "", 28, 21, 100.0),
            ("src/b.ts", "", 32, 21, 200.0),
            // The nearest line wins over the nearest column.
            ("src/c.ts", "", 5, 40, 100.0),
            ("src/c.ts", "", 6, 10, 200.0),
            // On one line, the nearest column.
            ("src/d.ts", "", 8, 4, 100.0),
            ("src/d.ts", "", 8, 30, 200.0),
        ],
    );
    let report = report(&raw(&folder, head, Some(before)));

    let found: Vec<(&str, u32, f64, Option<Verdict>)> = report
        .hot
        .iter()
        .map(|hot| {
            let before = hot.before.as_ref().map(|before| before.verdict);
            (hot.file.as_str(), hot.line, hot.self_ms, before)
        })
        .collect();
    let within = Some(Verdict::WithinNoise);
    let expected = [
        ("src/a.ts", 50, 100.0, Some(Verdict::Improved)),
        ("src/b.ts", 30, 100.0, within),
        ("src/c.ts", 5, 100.0, within),
        ("src/d.ts", 8, 100.0, within),
        ("src/a.ts", 10, 50.0, within),
    ];
    assert_eq!(found, expected);
}

#[test]
fn coverage_adds_up_its_services_and_a_failed_repeat_has_none() {
    let folder = Folder::new();
    let mut raw = raw(&folder, repeats(&folder, "head", &[100]), None);
    let api = vec![called("src/f.ts", 1, 1, 2)];
    let worker = vec![
        called("src/f.ts", 1, 1, 3),
        called("src/f.ts", 1, 9, 4),
        called("src/g.ts", 5, 1, 1),
    ];
    raw.coverage = Some(CoverageOutcome::Read(BTreeMap::from([
        ("api".to_owned(), Coverage { called: api }),
        ("worker".to_owned(), Coverage { called: worker }),
    ])));
    let coverage = report(&raw).coverage.expect("coverage");

    assert_eq!(coverage.services, ["api", "worker"]);
    let called = coverage.called.iter();
    let calls: Vec<(&str, u32, u32, u64)> = called
        .map(|called| {
            (
                called.file.as_str(),
                called.line,
                called.column,
                called.calls,
            )
        })
        .collect();
    let expected = [
        ("src/f.ts", 1, 1, 5),
        ("src/f.ts", 1, 9, 4),
        ("src/g.ts", 5, 1, 1),
    ];
    assert_eq!(calls, expected);
    let json = serde_json::to_value(&coverage).unwrap();
    let first = json!({"file": "src/f.ts", "line": 1, "column": 1, "calls": 5});
    assert_eq!(json["called"][0], first);

    raw.coverage = Some(CoverageOutcome::Failed(Failure {
        phase: Phase::Workload,
        service: Some("load".to_owned()),
        repeat: None,
        cause: Cause::Timeout,
        logs: None,
        probes: Vec::new(),
    }));
    assert!(report(&raw).coverage.is_none());
}

#[test]
fn a_service_with_no_call_and_no_profile_leaves_the_coverage_unknown() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[100, 100]);
    head[0].instances[0].profile = Some(read(Vec::new()));
    head[1].instances[0].profile = Some(ProfileOutcome::NoProfile);
    let mut raw = raw(&folder, head, None);
    let worker = Coverage {
        called: vec![called("src/w.ts", 3, 1, 7)],
    };
    raw.coverage = Some(CoverageOutcome::Read(BTreeMap::from([
        ("api".to_owned(), Coverage::default()),
        ("worker".to_owned(), worker),
    ])));
    // `api` may have written no chunk: `stop-slop hot` must not read its files as never run.
    assert!(report(&raw).coverage.is_none());

    // A call shows the coverage of `api` was read, whatever its measured repeats wrote.
    if let Some(CoverageOutcome::Read(services)) = &mut raw.coverage {
        let api = services.get_mut("api").expect("api");
        api.called.push(called("src/a.ts", 1, 1, 1));
    }
    assert!(report(&raw).coverage.is_some());

    // With a profile in every measured repeat, no call is a fact: no function of `api` ran.
    raw.head[1].instances[0].profile = Some(read(Vec::new()));
    if let Some(CoverageOutcome::Read(services)) = &mut raw.coverage {
        services.insert("api".to_owned(), Coverage::default());
    }
    let report = report(&raw);
    let coverage = report.coverage.as_ref().expect("coverage");
    assert_eq!(coverage.services, ["api", "worker"]);
    assert_eq!(coverage.called.len(), 1);
}
