//! `read_outputs` and `make` on values of `Raw` and `Context` (blueprint, Tests `report.rs`). No
//! VM: a temporary folder stands in for the run folder that `run` leaves.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::time::{Duration, SystemTime};

use camino::{Utf8Path, Utf8PathBuf};
use profile::{Called, Coverage, ModuleTime, Sampled, Samples};
use report::{
    At, BeforeInfo, BeforeOutcome, Behaviour, Bound, CategoryChange, Cause, Context,
    CoverageOutcome, DeadReason, Declared, Diff, Direction, ExpectJson, Extra, Failure, FaultJson,
    FaultOutcome, FaultRepeat, Flag, Grade, InstanceOutput, Intent, Journal, Line, Measure, Need,
    Next, Pass, PassFile, PassPair, PassState, PastRun, Phase, PostHoc, Previous, ProbeRan,
    ProbeRule, ProfileOutcome, Raw, RepeatOutput, Report, ReportError, Role, SealInfo,
    SecondPassOutcome, Setup, SetupFrom, SetupSide, Src, Topic, Unit, Verdict, load, make,
    read_outputs,
};
use sandbox::{ExecOutcome, Metrics, PortStats, Stopped, Unexposed};
use serde_json::{Value, json};
use tempfile::TempDir;

const GIB: u64 = 1 << 30;
const RUN: &str = "20261022-100000";
const PREVIOUS: &str = "20261022-094500";
const HEAD_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const BEFORE_TREEISH: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";
const BEFORE_TREE: &str = "7d1e09f3c2b4a5968778695a4b3c2d1e0f9a8b7c";
const DIGEST: &str = "9f2c41aa6b0e4d3c8a7f5e1d2c4b6a8f0e9d7c5b3a1f2e4d6c8b0a9f7e5d3c1b";
const FIXTURE: &str = include_str!("../../../../shared/fixtures/report.json");

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
    plain_with(folder, side, repeat, wall, &[])
}

/// `files` in its `out/`.
fn plain_with(
    folder: &Folder,
    side: &str,
    repeat: u32,
    wall: u64,
    files: &[(&str, &str)],
) -> RepeatOutput {
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
        out_dir: folder.out(&dir, files),
    }
}

/// One repeat per wall time.
fn repeats(folder: &Folder, side: &str, walls: &[u64]) -> Vec<RepeatOutput> {
    let numbered = walls.iter().zip(1..);
    numbered
        .map(|(wall, repeat)| plain(folder, side, repeat, *wall))
        .collect()
}

/// One repeat per list of files in its `out/`.
fn with_files(folder: &Folder, side: &str, files: &[&[(&str, &str)]]) -> Vec<RepeatOutput> {
    let numbered = files.iter().zip(1..);
    numbered
        .map(|(files, repeat)| plain_with(folder, side, repeat, 1000, files))
        .collect()
}

fn setup_side(folder: &Folder, side: &str, files: &[(&str, &str)]) -> SetupSide {
    SetupSide {
        sizes: BTreeMap::new(),
        probes: Vec::new(),
        out_dir: folder.out(&format!("setup-{side}"), files),
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
            head: setup_side(folder, "head", &[]),
            before: before.as_ref().map(|_| setup_side(folder, "before", &[])),
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
        changed: None,
        before,
        seal: None,
        history: Vec::new(),
        declared: Vec::new(),
        accepted: Vec::new(),
        previous,
        pool,
        setup_from: SetupFrom::default(),
        reused: BTreeSet::new(),
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

/// A passed run of the pool, written as `cli` writes it, with its folder.
fn written(folder: &Folder, name: &str, raw: &Raw, context: &Context<'_>) -> (Report, Utf8PathBuf) {
    let report = report_of(raw, context, &[]);
    let dir = folder.path().join(name);
    fs::create_dir_all(&dir).expect("create the run folder");
    report.write(&dir).expect("the report writes");
    (report, dir)
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

fn behaviour<'r>(report: &'r Report, file: &str) -> &'r Behaviour {
    let files = report.behaviour.as_ref().expect("a behaviour section");
    let found = files.iter().find(|found| found.file == file);
    found.unwrap_or_else(|| panic!("no file {file}"))
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
    raw.setup.before = Some(setup_side(&folder, "before", &[]));
    raw.before = Some(BeforeOutcome::Failed(failure.clone()));
    let report = report(&raw);

    let before = report.before.as_ref().expect("a before");
    assert_eq!(before.failed.as_ref(), Some(&failure));
    let wall = measure(&report, "stand.wall");
    let wall = wall.before.as_ref().expect("a verdict against before");
    assert_eq!((wall.median, wall.verdict), (None, Verdict::NoBaseline));
    assert!(wall.samples.is_empty());
    assert!(report.behaviour.is_none());
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
            stage: Some(Phase::Ready),
            service: None,
        },
        // Both ran and wrote no `probe.<name>.jsonl`.
        Flag::DeadProbe {
            probe: "orders".to_owned(),
            reason: DeadReason::NoRecords,
        },
        Flag::DeadProbe {
            probe: "seed".to_owned(),
            reason: DeadReason::NoRecords,
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
    raw.setup.before = Some(setup_side(&folder, "before", &[]));
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
    let failed = report(&raw);
    assert!(failed.coverage.is_none());
    let unknown = Flag::CoverageUnknown {
        stage: Some(Phase::Workload),
        service: None,
    };
    assert_eq!(unknowns(&failed), [&unknown]);
    let json = serde_json::to_value(&unknown).unwrap();
    let expected = json!({"kind": "coverage-unknown", "stage": "workload", "service": null});
    assert_eq!(json, expected);
    assert_eq!(serde_json::from_value::<Flag>(json).unwrap(), unknown);
}

fn unknowns(report: &Report) -> Vec<&Flag> {
    let flags = report.flags.iter();
    let unknown = |flag: &&Flag| matches!(flag, Flag::CoverageUnknown { .. });
    flags.filter(unknown).collect()
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
    let unseen = report(&raw);
    assert!(unseen.coverage.is_none());
    let unknown = Flag::CoverageUnknown {
        stage: None,
        service: Some("api".to_owned()),
    };
    assert_eq!(unknowns(&unseen), [&unknown]);
    let json = serde_json::to_value(&unknown).unwrap();
    let expected = json!({"kind": "coverage-unknown", "stage": null, "service": "api"});
    assert_eq!(json, expected);

    // A call shows the coverage of `api` was read, whatever its measured repeats wrote.
    if let Some(CoverageOutcome::Read(services)) = &mut raw.coverage {
        let api = services.get_mut("api").expect("api");
        api.called.push(called("src/a.ts", 1, 1, 1));
    }
    let called_once = report(&raw);
    assert!(called_once.coverage.is_some());
    assert!(unknowns(&called_once).is_empty());

    // With a profile in every measured repeat, no call is a fact: no function of `api` ran.
    raw.head[1].instances[0].profile = Some(read(Vec::new()));
    if let Some(CoverageOutcome::Read(services)) = &mut raw.coverage {
        services.insert("api".to_owned(), Coverage::default());
    }
    let report = report(&raw);
    let coverage = report.coverage.as_ref().expect("coverage");
    assert_eq!(coverage.services, ["api", "worker"]);
    assert_eq!(coverage.called.len(), 1);
    assert!(unknowns(&report).is_empty());
}

#[test]
fn a_coverage_repeat_of_no_service_leaves_the_coverage_unknown() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[100]);
    head[0].instances[0].profile = Some(read(Vec::new()));
    let mut raw = raw(&folder, head, None);
    raw.coverage = Some(CoverageOutcome::Read(BTreeMap::new()));
    let report = report(&raw);

    assert!(report.coverage.is_none());
    let unknown = Flag::CoverageUnknown {
        stage: None,
        service: None,
    };
    assert_eq!(unknowns(&report), [&unknown]);
}

// --- behaviour, records, write and load --------------------------------------------------------

#[test]
fn fields_that_differ_between_repeats_of_before_are_noise() {
    let folder = Folder::new();
    let head: &[&[(&str, &str)]] = &[
        &[("out.jsonl", "{\"t\":11,\"total\":11}\n")],
        &[("out.jsonl", "{\"t\":12,\"total\":11}\n")],
    ];
    let before: &[&[(&str, &str)]] = &[
        &[("out.jsonl", "{\"t\":1,\"total\":10}\n")],
        &[("out.jsonl", "{\"t\":2,\"total\":10}\n")],
    ];
    let head = with_files(&folder, "head", head);
    let before = with_files(&folder, "before", before);
    let report = report(&raw(&folder, head, Some(before)));

    let file = behaviour(&report, "out.jsonl");
    assert_eq!((file.src, file.key.as_deref()), (Src::Workload, None));
    assert!(file.noise_known);
    assert_eq!(file.noise, ["/t"]);
    // `/t` changes on the head too, and it is noise there as well.
    assert!(file.head_only_noise.is_empty());
    let diffs: Vec<(&At, &str, &Value, &Value)> = file
        .diffs
        .iter()
        .map(|diff| (&diff.at, diff.path.as_str(), &diff.before, &diff.head))
        .collect();
    let total = (&At::Record { record: 1 }, "/total", &json!(10), &json!(11));
    assert_eq!(diffs, [total]);
}

#[test]
fn one_repeat_of_before_leaves_the_noise_unknown() {
    let folder = Folder::new();
    let head = with_files(&folder, "head", &[&[("out.jsonl", "{\"t\":11}\n")]]);
    let before = with_files(&folder, "before", &[&[("out.jsonl", "{\"t\":1}\n")]]);
    let report = report(&raw(&folder, head, Some(before)));

    let file = behaviour(&report, "out.jsonl");
    assert!(!file.noise_known);
    assert!(file.noise.is_empty());
    assert_eq!(file.diffs.len(), 1);
}

fn no_results(report: &Report) -> Vec<&Flag> {
    let flags = report.flags.iter();
    flags
        .filter(|flag| matches!(flag, Flag::NoResults { .. }))
        .collect()
}

#[test]
fn a_file_on_one_side_or_no_file_at_all_is_no_results() {
    let folder = Folder::new();
    let files = [("out.jsonl", "{}\n"), ("extra.jsonl", "{}\n")];
    let head = with_files(&folder, "head", &[&files]);
    let before = with_files(&folder, "before", &[&[("out.jsonl", "{}\n")]]);
    let one = report(&raw(&folder, head, Some(before)));
    let extra = Flag::NoResults {
        file: Some("extra.jsonl".to_owned()),
    };
    assert_eq!(no_results(&one), [&extra]);
    // Its records still show, each as a record of one side.
    let diff = &behaviour(&one, "extra.jsonl").diffs[0];
    assert_eq!((diff.path.as_str(), &diff.before), ("", &Value::Null));

    let head = repeats(&folder, "a", &[100]);
    let empty = report(&raw(&folder, head, Some(repeats(&folder, "b", &[100]))));
    assert_eq!(no_results(&empty), [&Flag::NoResults { file: None }]);
    assert_eq!(empty.behaviour.as_ref().map(Vec::len), Some(0));

    // Without `--before` nothing is compared, so no file is missing.
    let alone = report(&raw(&folder, repeats(&folder, "c", &[100]), None));
    assert!(no_results(&alone).is_empty());
    assert!(alone.behaviour.is_none());
}

#[test]
fn a_broken_line_of_a_workload_file_is_flagged_and_left_out() {
    let folder = Folder::new();
    let broken = "{\"a\":1}\n\nnot json\nnor this\n";
    let head = with_files(&folder, "head", &[&[("out.jsonl", broken)]]);
    let before = with_files(&folder, "before", &[&[("out.jsonl", "{\"a\":1}\n")]]);
    let report = report(&raw(&folder, head, Some(before)));

    // The first broken line, the blank line counted.
    let bad = Flag::BadResultLine {
        file: "out.jsonl".to_owned(),
        line: 3,
    };
    assert!(report.flags.contains(&bad), "{:?}", report.flags);
    assert!(behaviour(&report, "out.jsonl").diffs.is_empty());
}

/// Repeats that run `probe` after the workload, one output each.
fn probing(folder: &Folder, side: &str, outputs: &[&str], probe: &str) -> Vec<RepeatOutput> {
    let file = format!("probe.{probe}.jsonl");
    let lists: Vec<[(&str, &str); 1]> = outputs
        .iter()
        .map(|text| [(file.as_str(), *text)])
        .collect();
    let lists: Vec<&[(&str, &str)]> = lists.iter().map(|list| list.as_slice()).collect();
    let mut repeats = with_files(folder, side, &lists);
    for repeat in &mut repeats {
        let out = repeat.out_dir.clone();
        let ran = probe_ran(probe, ExecOutcome::Exited(0), &out);
        repeat.probes.push(ran);
    }
    repeats
}

fn probe_rule(raw: &mut Raw, probe: &str, key: Option<&str>, ignore: &[&str]) {
    let rule = ProbeRule {
        key: key.map(str::to_owned),
        ignore: ignore.iter().map(|path| (*path).to_owned()).collect(),
    };
    raw.probes.insert(format!("probe.{probe}.jsonl"), rule);
}

#[test]
fn a_probe_line_that_is_not_json_compares_as_a_line_record() {
    let folder = Folder::new();
    let head = probing(&folder, "head", &["load 2\n"], "top");
    let before = probing(&folder, "before", &["load 1\n"], "top");
    let mut raw = raw(&folder, head, Some(before));
    probe_rule(&mut raw, "top", None, &[]);
    let report = report(&raw);

    let file = behaviour(&report, "probe.top.jsonl");
    assert_eq!(file.src, Src::Probe);
    let diff = &file.diffs[0];
    let found = (diff.path.as_str(), &diff.before, &diff.head);
    assert_eq!(found, ("/line", &json!("load 1"), &json!("load 2")));
    let mut flags = report.flags.iter();
    assert!(!flags.any(|flag| matches!(flag, Flag::BadResultLine { .. })));
}

#[test]
fn a_keyed_probe_pairs_records_by_its_key_and_shows_a_record_of_one_side() {
    let folder = Folder::new();
    let before =
        "{\"id\":7,\"status\":\"paid\",\"updated_at\":\"a\"}\n{\"id\":8,\"status\":\"paid\"}\n";
    let head = "{\"id\":8,\"status\":\"paid\"}\n{\"id\":7,\"status\":\"pending\",\"updated_at\":\"b\"}\n{\"id\":9}\n";
    let head = probing(&folder, "head", &[head], "orders");
    let before = probing(&folder, "before", &[before], "orders");
    let mut raw = raw(&folder, head, Some(before));
    probe_rule(&mut raw, "orders", Some("/id"), &["/updated_at"]);
    let report = report(&raw);

    let file = behaviour(&report, "probe.orders.jsonl");
    assert_eq!(file.key.as_deref(), Some("/id"));
    let diffs: Vec<(&At, &str)> = file
        .diffs
        .iter()
        .map(|diff| (&diff.at, diff.path.as_str()))
        .collect();
    let expected = [
        (&At::Key { key: json!(7) }, "/status"),
        (&At::Key { key: json!(9) }, ""),
    ];
    assert_eq!(diffs, expected);
    assert_eq!(file.diffs[1].before, Value::Null);
}

#[test]
fn a_probe_without_records_or_with_every_field_ignored_is_dead() {
    let folder = Folder::new();
    let mut head = probing(&folder, "head", &[""], "empty");
    let ignored = probing(&folder, "clock", &["{\"at\":1}\n"], "clock");
    head[0].probes.extend(ignored[0].probes.clone());
    let clock = "probe.clock.jsonl";
    let copied = fs::copy(ignored[0].out_dir.join(clock), head[0].out_dir.join(clock));
    copied.expect("copy the output of clock");
    let mut raw = raw(&folder, head, None);
    probe_rule(&mut raw, "empty", None, &[]);
    probe_rule(&mut raw, "clock", None, &["/at"]);
    // A probe of the recipe that never ran, as an `on-failure` one of a passed run.
    probe_rule(&mut raw, "rescue", None, &[]);
    let report = report(&raw);

    let flags = report.flags.iter();
    let dead: Vec<&Flag> = flags
        .filter(|flag| matches!(flag, Flag::DeadProbe { .. }))
        .collect();
    let expected = [
        &Flag::DeadProbe {
            probe: "clock".to_owned(),
            reason: DeadReason::AllIgnored,
        },
        &Flag::DeadProbe {
            probe: "empty".to_owned(),
            reason: DeadReason::NoRecords,
        },
    ];
    assert_eq!(dead, expected);
}

#[test]
fn noise_only_on_the_head_is_flagged_and_stays_in_the_diff() {
    let folder = Folder::new();
    let head: &[&[(&str, &str)]] = &[
        &[("out.jsonl", "{\"x\":1,\"y\":2}\n")],
        &[("out.jsonl", "{\"x\":1,\"y\":3}\n")],
    ];
    let before: &[&[(&str, &str)]] = &[
        &[("out.jsonl", "{\"x\":1,\"y\":1}\n")],
        &[("out.jsonl", "{\"x\":1,\"y\":1}\n")],
    ];
    let head = with_files(&folder, "head", head);
    let before = with_files(&folder, "before", before);
    let report = report(&raw(&folder, head, Some(before)));

    let flag = Flag::HeadOnlyNoise {
        file: "out.jsonl".to_owned(),
        path: "/y".to_owned(),
    };
    assert!(report.flags.contains(&flag));
    let file = behaviour(&report, "out.jsonl");
    assert_eq!(file.head_only_noise, ["/y"]);
    assert_eq!(file.diffs[0].path, "/y");
}

/// A run whose `/seed` holds through the two repeats of each version, its values (before, head).
fn seeded(folder: &Folder, name: &str, (before, head): (u32, u32)) -> Raw {
    let side = |side: &str, seed: u32| {
        let line = format!("{{\"seed\":{seed}}}\n");
        let files = [("out.jsonl", line.as_str())];
        with_files(folder, &format!("{name}-{side}"), &[&files, &files])
    };
    raw(folder, side("head", head), Some(side("before", before)))
}

#[test]
fn the_pool_teaches_the_noise_mask_through_records_json() {
    let folder = Folder::new();
    let earlier = seeded(&folder, "a", (1, 2));
    let mut earlier_context = context(&earlier, Previous::None, &[]);
    earlier_context.run = PREVIOUS.to_owned();
    let (pooled, dir) = written(&folder, "run-a", &earlier, &earlier_context);
    let raw = seeded(&folder, "b", (3, 4));

    let alone = report(&raw);
    assert_eq!(behaviour(&alone, "out.jsonl").diffs[0].path, "/seed");
    let pool = [pooled];
    let dirs = [dir.clone()];
    let together = report_of(&raw, &context(&raw, Previous::None, &pool), &dirs);
    let file = behaviour(&together, "out.jsonl");
    assert_eq!(file.noise, ["/seed"]);
    assert!(file.diffs.is_empty());

    // A pooled run whose `records.json` does not read teaches nothing and stops nothing.
    fs::write(dir.join("records.json"), "{").expect("break the records");
    let unread = report_of(&raw, &context(&raw, Previous::None, &pool), &dirs);
    assert_eq!(behaviour(&unread, "out.jsonl").diffs[0].path, "/seed");
}

#[test]
fn after_setup_records_of_one_setup_origin_count_once() {
    let folder = Folder::new();
    let run = |name: &str, value: u32, origin: &str, id: &str| {
        let head = repeats(&folder, &format!("{name}-head"), &[100]);
        let before = repeats(&folder, &format!("{name}-before"), &[100]);
        let mut raw = raw(&folder, head, Some(before));
        let text = format!("{{\"v\":{value}}}\n");
        let files = [("probe.seed.jsonl", text.as_str())];
        raw.setup.before = Some(setup_side(&folder, &format!("{name}-before"), &files));
        let files = [("probe.seed.jsonl", "{\"v\":9}\n")];
        raw.setup.head = setup_side(&folder, &format!("{name}-head"), &files);
        probe_rule(&mut raw, "seed", None, &[]);
        let mut context = context(&raw, Previous::None, &[]);
        context.run = id.to_owned();
        context.setup_from.before = Some(origin.to_owned());
        (raw, context)
    };
    let pool_of = |origins: [&str; 2]| {
        let runs = [
            run("a", 1, origins[0], "20261022-090000"),
            run("b", 2, origins[1], "20261022-091000"),
        ];
        let written = runs
            .iter()
            .map(|(raw, context)| written(&folder, &context.run, raw, context));
        written.unzip::<_, _, Vec<Report>, Vec<Utf8PathBuf>>()
    };
    let (raw, mut this) = run("c", 3, "x", RUN);

    // Every "before" restored the setup that one run built: one list of its records.
    let (one, dirs) = pool_of(["x", "x"]);
    this.pool = &one;
    let same = behaviour(&report_of(&raw, &this, &dirs), "probe.seed.jsonl").clone();
    assert!(!same.noise_known);
    assert!(same.noise.is_empty());
    assert_eq!(same.diffs.len(), 1);

    let (two, dirs) = pool_of(["x", "y"]);
    this.pool = &two;
    let apart = behaviour(&report_of(&raw, &this, &dirs), "probe.seed.jsonl").clone();
    assert!(apart.noise_known);
    assert_eq!(apart.noise, ["/v"]);
    assert!(apart.diffs.is_empty());
}

#[test]
fn read_outputs_takes_only_the_jsonl_files_of_each_out_folder() {
    let folder = Folder::new();
    let files: &[(&str, &str)] = &[
        ("out.jsonl", "{}\n"),
        ("probe.orders.jsonl", "{}\n"),
        ("probe.orders.jsonl.err", "psql: warning\n"),
        (
            "probe.orders.jsonl.status",
            "{\"code\":0,\"timedOut\":false}\n",
        ),
        ("notes.txt", "a note of a person\n"),
    ];
    let head = with_files(&folder, "head", &[files]);
    let nested = head[0].out_dir.join("nested");
    fs::create_dir(&nested).expect("a folder in out");
    fs::write(nested.join("deep.jsonl"), "{}\n").expect("a file below out");
    let before = with_files(&folder, "before", &[files]);
    let mut raw = raw(&folder, head, Some(before));
    probe_rule(&mut raw, "orders", None, &[]);
    let report = report(&raw);

    let behaviour = report.behaviour.iter().flatten();
    let files: Vec<&str> = behaviour.map(|file| file.file.as_str()).collect();
    assert_eq!(files, ["out.jsonl", "probe.orders.jsonl"]);
    let mut flags = report.flags.iter();
    assert!(!flags.any(|flag| matches!(flag, Flag::BadResultLine { .. })));
}

#[test]
fn a_folder_named_jsonl_in_out_is_no_input() {
    let folder = Folder::new();
    let head = with_files(&folder, "head", &[&[("out.jsonl", "{}\n")]]);
    let odd = head[0].out_dir.join("odd.jsonl");
    fs::create_dir(&odd).expect("a folder in out");
    let raw = raw(&folder, head, None);

    let error = read_outputs(&raw, &[]).expect_err("a folder is no result file");
    assert!(
        matches!(&error, ReportError::Bad { path, .. } if *path == odd),
        "{error}"
    );
}

/// The guest writes `/stand/out`: a link there would read the host. Windows here gives no right
/// to make links, so Linux CI runs this.
#[cfg(unix)]
#[test]
fn a_link_in_out_is_never_followed() {
    let folder = Folder::new();
    let secret = folder.path().join("secret.jsonl");
    fs::write(&secret, "{\"token\":\"of the host\"}\n").expect("a file of the host");
    let head = with_files(&folder, "head", &[&[("out.jsonl", "{}\n")]]);
    let out = head[0].out_dir.clone();
    // A link of another name is no result file: it is never opened.
    std::os::unix::fs::symlink(&secret, out.join("leak")).expect("a link");
    let raw = raw(&folder, head, None);
    assert!(read_outputs(&raw, &[]).is_ok());

    let link = out.join("leak.jsonl");
    std::os::unix::fs::symlink(&secret, &link).expect("a link");
    let error = read_outputs(&raw, &[]).expect_err("a link is no result file");
    assert!(
        matches!(&error, ReportError::Bad { path, .. } if *path == link),
        "{error}"
    );
}

#[test]
fn the_on_failure_probes_of_a_failed_before_are_its_failure_not_a_side() {
    let folder = Folder::new();
    let head = probing(&folder, "head", &["{\"n\":1}\n", "{\"n\":1}\n"], "orders");
    let mut raw = raw(&folder, head, None);
    probe_rule(&mut raw, "orders", None, &[]);
    probe_rule(&mut raw, "rescue", None, &[]);
    let output = [("probe.rescue.jsonl", "{\"state\":\"stuck\"}\n")];
    let mut setup = setup_side(&folder, "before", &output);
    let rescue = probe_ran("rescue", ExecOutcome::Exited(3), &setup.out_dir);
    setup.probes.push(rescue.clone());
    raw.setup.before = Some(setup);
    raw.before = Some(BeforeOutcome::Failed(Failure {
        phase: Phase::Setup,
        service: Some("db".to_owned()),
        repeat: None,
        cause: Cause::Unhealthy,
        logs: None,
        probes: vec![rescue.out],
    }));
    let context = context(&raw, Previous::None, &[]);
    let (report, dir) = written(&folder, "run", &raw, &context);

    assert!(report.behaviour.is_none());
    let rescued = |flag: &Flag| match flag {
        Flag::DeadProbe { probe, .. } | Flag::ProbeFailed { probe, .. } => probe == "rescue",
        _ => false,
    };
    assert!(!report.flags.iter().any(rescued), "{:?}", report.flags);
    // Nor do the later runs of the pool learn from it.
    let records = fs::read_to_string(dir.join("records.json")).expect("records.json");
    assert!(!records.contains("stuck"), "{records}");
    assert!(records.contains("\"n\":1"), "{records}");
}

#[test]
fn write_puts_the_report_its_diffs_and_records_and_load_reads_the_report_back() {
    let folder = Folder::new();
    let orders = |status: &str| format!("{{\"id\":7,\"status\":\"{status}\"}}\n");
    let (paid, pending) = (orders("paid"), orders("pending"));
    let head = [
        ("out.jsonl", "{\"total\":11}\n"),
        ("probe.orders.jsonl", pending.as_str()),
    ];
    let before = [
        ("out.jsonl", "{\"total\":10}\n"),
        ("probe.orders.jsonl", paid.as_str()),
    ];
    let head = with_files(&folder, "head", &[&head]);
    let before = with_files(&folder, "before", &[&before]);
    let mut raw = raw(&folder, head, Some(before));
    probe_rule(&mut raw, "orders", Some("/id"), &[]);
    let context = context(&raw, Previous::None, &[]);
    let (written, dir) = written(&folder, "run", &raw, &context);

    let files = [
        "report.json",
        "records.json",
        "diff/out.jsonl.json",
        "diff/probe.orders.jsonl.json",
    ];
    for file in files {
        assert!(dir.join(file).is_file(), "{file}");
    }
    assert!(!dir.join("report.json.tmp").exists());
    let read = |file: &str| -> Value {
        let text = fs::read_to_string(dir.join(file)).expect("a written file");
        serde_json::from_str(&text).expect("JSON")
    };
    let total = json!([{"record": 1, "path": "/total", "before": 10, "head": 11}]);
    assert_eq!(read("diff/out.jsonl.json"), total);
    let status = json!([{"key": 7, "path": "/status", "before": "paid", "head": "pending"}]);
    assert_eq!(read("diff/probe.orders.jsonl.json"), status);
    let records = fs::read_to_string(dir.join("records.json")).expect("records.json");
    assert!(!records.contains('\n'), "compact: {records}");

    let loaded = load(&dir).expect("the report loads");
    let json = |report: &Report| serde_json::to_value(report).expect("a report serializes");
    assert_eq!(json(&loaded), json(&written));
    let keyed = &behaviour(&loaded, "probe.orders.jsonl").diffs[0];
    assert_eq!(keyed.at, At::Key { key: json!(7) });
}

#[test]
fn load_refuses_another_schema_version_and_a_broken_report() {
    let folder = Folder::new();
    let raw = raw(&folder, repeats(&folder, "head", &[100]), None);
    let (_, dir) = written(&folder, "run", &raw, &context(&raw, Previous::None, &[]));
    let path = dir.join("report.json");
    let text = fs::read_to_string(&path).expect("report.json");

    let other = text.replacen("\"schemaVersion\": 1", "\"schemaVersion\": 2", 1);
    fs::write(&path, other).expect("write another version");
    let refused = load(&dir);
    assert!(
        matches!(refused, Err(ReportError::Schema { found: 2, .. })),
        "{refused:?}"
    );
    fs::write(&path, "{\"schemaVersion\": 1, \"run\": ").expect("break the report");
    assert!(matches!(load(&dir), Err(ReportError::Bad { .. })));
    let missing = load(&folder.path().join("nowhere"));
    assert!(matches!(missing, Err(ReportError::Io { .. })));
}

// --- the line and the summary ------------------------------------------------------------------

fn line_files(line: &Line) -> Vec<(&str, u32, usize)> {
    let files = line.behaviour.iter().flatten();
    let files = files.map(|file| (file.file.as_str(), file.diffs, file.sample.len()));
    files.collect()
}

/// A path as the report and the line print it.
fn slashed(path: &Utf8Path) -> String {
    path.as_str().replace('\\', "/")
}

fn first_line(report: &Report) -> String {
    let summary = report.summary().to_string();
    summary.lines().next().unwrap_or_default().to_owned()
}

#[test]
fn the_quiet_line_holds_only_the_exceptions_and_reads_no_file() {
    let folder = Folder::new();
    let many: String = (1..=5)
        .map(|total| format!("{{\"total\":{total}}}\n"))
        .collect();
    let none: String = (1..=5).map(|_| "{\"total\":0}\n").collect();
    // `broken.jsonl` has no diff, but a flag names it.
    let head = [
        ("out.jsonl", many.as_str()),
        ("same.jsonl", "{}\n"),
        ("broken.jsonl", "{}\nnot json\n"),
    ];
    let before = [
        ("out.jsonl", none.as_str()),
        ("same.jsonl", "{}\n"),
        ("broken.jsonl", "{}\n"),
    ];
    let head = with_files(&folder, "head", &[&head]);
    let before = with_files(&folder, "before", &[&before]);
    let mut raw = raw(&folder, head, Some(before));
    raw.head[0].wall = Duration::from_millis(2000);
    let report = report(&raw);
    let dir = Utf8Path::new("nowhere").join("run");
    let line = report.line(&dir);

    let names: Vec<&str> = line
        .measures
        .iter()
        .map(|measure| measure.name.as_str())
        .collect();
    assert_eq!(names, ["stand.wall"]);
    assert_eq!(
        line_files(&line),
        [("broken.jsonl", 0, 0), ("out.jsonl", 5, 3)]
    );
    let out = &line.behaviour.as_ref().expect("files")[1];
    assert_eq!(out.sample, behaviour(&report, "out.jsonl").diffs[..3]);
    assert_eq!(out.path, dir.join("diff").join("out.jsonl.json"));
    // Every measure but `stand.wall`, and `same.jsonl`.
    assert_eq!(line.quiet as usize, report.measures.len() - 1 + 1);
    assert_eq!(line.run.as_deref(), Some(RUN));
    assert_eq!(line.report, Some(dir.join("report.json")));
    assert_eq!(line.flags, report.flags);
    assert!(line.failure.is_none() && line.next.is_empty());
    let json = serde_json::to_value(&line).unwrap();
    assert!(
        json.get("next").is_none(),
        "cli prints `next`, not the line"
    );
    assert_eq!(json["report"], "nowhere/run/report.json");
    assert_eq!(
        json["behaviour"][1]["path"],
        "nowhere/run/diff/out.jsonl.json"
    );
    assert_eq!(json["unmeasurable"], 0);
    assert!(json["measures"][0].get("samples").is_none());
}

#[test]
fn without_before_a_shift_against_the_previous_run_is_an_exception() {
    let folder = Folder::new();
    let previous = report(&raw(
        &folder,
        repeats(&folder, "previous", &[100, 100, 100]),
        None,
    ));
    let raw = raw(&folder, repeats(&folder, "head", &[200, 200, 200]), None);
    let report = report_of(&raw, &context(&raw, Previous::Read(&previous), &[]), &[]);

    let line = report.line(Utf8Path::new("run"));
    let names: Vec<&str> = line
        .measures
        .iter()
        .map(|measure| measure.name.as_str())
        .collect();
    assert_eq!(names, ["stand.wall"]);
    assert_eq!(line.measures[0].previous.verdict, Verdict::Regressed);
    assert!(line.measures[0].before.is_none() && line.behaviour.is_none());
    assert_eq!(line.quiet as usize, report.measures.len() - 1);
}

#[test]
fn a_failed_line_carries_the_facts_so_far_and_its_next_steps() {
    // The head fails in its second repeat: `schema` after setup and `orders` after the first
    // workload exited with a code, and `rescue` ran on the failure.
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[100, 100]);
    let first = head[0].out_dir.clone();
    head[0]
        .probes
        .push(probe_ran("orders", ExecOutcome::Exited(2), &first));
    let failed = head[1].out_dir.clone();
    let rescue = probe_ran("rescue", ExecOutcome::Exited(3), &failed);
    head[1].probes.push(rescue.clone());
    head[1].instances[0].metrics.unexposed.push(Unexposed {
        name: "cache".to_owned(),
        attempts: 2,
    });
    let mut raw = raw(&folder, head, None);
    let setup_out = raw.setup.head.out_dir.clone();
    let schema = probe_ran("schema", ExecOutcome::Exited(1), &setup_out);
    raw.setup.head.probes.push(schema);
    let logs = folder.path().join("repeat-2-head");
    let failure = Failure {
        phase: Phase::Workload,
        service: Some("load".to_owned()),
        repeat: Some(2),
        cause: Cause::Exit(1),
        logs: Some(logs.clone()),
        probes: vec![rescue.out.clone()],
    };
    let next = vec![Next::Inspect {
        run: RUN.to_owned(),
        service: "db".to_owned(),
    }];
    let line = Line::failed(failure.clone(), Some(&raw), next.clone());

    let probe_failed = |probe: &str, repeat, code| Flag::ProbeFailed {
        probe: probe.to_owned(),
        side: "head".to_owned(),
        repeat,
        code,
    };
    // The failed second repeat gave no sample.
    let expected = [
        Flag::LowRepeats { samples: 1 },
        Flag::Unprofiled,
        Flag::MissingExpose {
            name: "cache".to_owned(),
            attempts: 2,
        },
        probe_failed("schema", None, Some(1)),
        probe_failed("orders", Some(1), Some(2)),
    ];
    assert_eq!(line.flags, expected);
    assert_eq!(line.failure.as_ref(), Some(&failure));
    assert_eq!(line.next, next);
    assert!(line.measures.is_empty() && line.behaviour.is_none());
    assert!(line.run.is_none() && line.report.is_none());
    let json = serde_json::to_value(&line).unwrap();
    assert_eq!(json["failure"]["logs"], slashed(&logs));
    assert!(json["run"].is_null() && json["report"].is_null());
    let text = line.summary().to_string();
    assert!(
        text.starts_with("failure workload load repeat 2: exit-1\n"),
        "{text}"
    );
    assert!(
        text.contains(&format!("\nlogs {}\n", slashed(&logs))),
        "{text}"
    );
    assert!(
        text.contains(&format!("\nprobes {}\n", slashed(&rescue.out))),
        "{text}"
    );

    // A step after `run` failed: both repeats passed, no probe ran on the failure, and `schema`
    // keeps its flag although the failure has no repeat either.
    raw.head[1].probes.clear();
    let after_run = Failure {
        phase: Phase::Report,
        service: None,
        repeat: None,
        cause: Cause::Error("repeat-1-head/out/out.jsonl: not a plain file".to_owned()),
        logs: None,
        probes: Vec::new(),
    };
    let line = Line::failed(after_run, Some(&raw), Vec::new());
    let mut expected = expected.to_vec();
    expected[0] = Flag::LowRepeats { samples: 2 };
    assert_eq!(line.flags, expected);

    // A failed setup: its probes ran on the failure too, and no repeat ran.
    raw.head.clear();
    let rescue = probe_ran("rescue", ExecOutcome::Exited(3), &setup_out);
    raw.setup.head.probes = vec![rescue.clone()];
    let failure = Failure {
        phase: Phase::Setup,
        service: Some("db".to_owned()),
        repeat: None,
        cause: Cause::Unhealthy,
        logs: Some(folder.path().join("setup-head")),
        probes: vec![rescue.out],
    };
    let line = Line::failed(failure.clone(), Some(&raw), Vec::new());
    assert!(line.flags.is_empty(), "{:?}", line.flags);

    let refused = Line::failed(failure, None, vec![Next::Init]);
    assert!(refused.flags.is_empty());
    assert_eq!(refused.next, [Next::Init]);
}

#[test]
fn the_line_and_the_summary_print_the_same_tokens() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[200, 200, 200]);
    head[0].instances[0].metrics.unexposed.push(Unexposed {
        name: "cache".to_owned(),
        attempts: 2,
    });
    let before = repeats(&folder, "before", &[100, 100, 100]);
    let raw = raw(&folder, head, Some(before));
    let report = report(&raw);
    let line = report.line(Utf8Path::new("run"));
    let texts = [report.summary().to_string(), line.summary().to_string()];

    let json = serde_json::to_value(&line).unwrap();
    let mut tokens = vec![
        "stand.wall 200 ms before 100 regressed",
        "missing-expose attempts 2 name cache",
    ];
    let kinds = json["flags"].as_array().unwrap().iter();
    tokens.extend(kinds.map(|flag| flag["kind"].as_str().unwrap()));
    let quiet = format!("\nquiet {}\n", line.quiet);
    tokens.push(&quiet);
    for text in &texts {
        for token in &tokens {
            assert!(text.contains(token), "{token} not in\n{text}");
        }
    }
    assert!(
        texts[1].ends_with("\nreport run/report.json\n"),
        "{}",
        texts[1]
    );
    assert!(!texts[0].contains("report.json"), "{}", texts[0]);
}

#[test]
fn the_summary_opens_with_no_comparison_and_coverage_unknown() {
    let folder = Folder::new();
    // Without a profiled service there is no coverage to miss: `unprofiled` stays a flag line.
    let plain = report(&raw(&folder, repeats(&folder, "plain", &[100]), None));
    assert_eq!(first_line(&plain), format!("run {RUN}"));
    let summary = plain.summary().to_string();
    assert!(
        summary.lines().any(|line| line == "unprofiled"),
        "{summary}"
    );

    let failed = |phase: Phase| Failure {
        phase,
        service: None,
        repeat: None,
        cause: Cause::Timeout,
        logs: None,
        probes: Vec::new(),
    };
    let mut raw = raw(&folder, repeats(&folder, "head", &[100, 100, 100]), None);
    raw.setup.before = Some(setup_side(&folder, "before", &[]));
    raw.before = Some(BeforeOutcome::Failed(failed(Phase::Setup)));
    raw.coverage = Some(CoverageOutcome::Failed(failed(Phase::Workload)));
    let summary = report(&raw).summary().to_string();
    let first: Vec<&str> = summary.lines().take(3).collect();
    let opening = [
        "NO COMPARISON: before failed at setup",
        "coverage unknown: repeat failed at workload",
    ];
    let run = format!("run {RUN}");
    assert_eq!(first, [opening[0], opening[1], run.as_str()]);
    let line = Line::failed(failed(Phase::Workload), Some(&raw), Vec::new());
    let text = line.summary().to_string();
    let opened = format!("{}\n{}\n", opening[0], opening[1]);
    assert!(text.starts_with(&opened), "{text}");

    // `api` wrote no profile in a measured repeat, and the coverage repeat saw no call of it.
    raw.before = None;
    raw.setup.before = None;
    raw.head[0].instances[0].profile = Some(ProfileOutcome::NoProfile);
    let services = BTreeMap::from([("api".to_owned(), Coverage::default())]);
    raw.coverage = Some(CoverageOutcome::Read(services));
    let unseen = "coverage unknown: api left no coverage";
    assert_eq!(first_line(&report(&raw)), unseen);
    raw.coverage = Some(CoverageOutcome::Read(BTreeMap::new()));
    let empty = "coverage unknown: no service in the coverage repeat";
    assert_eq!(first_line(&report(&raw)), empty);
}

#[test]
fn the_summary_names_the_setup_origin_and_the_line_does_not() {
    let folder = Folder::new();
    let raw = raw(
        &folder,
        repeats(&folder, "head", &[100]),
        Some(repeats(&folder, "before", &[100])),
    );
    let mut context = context(&raw, Previous::None, &[]);
    context.setup_from.before = Some(PREVIOUS.to_owned());
    let report = report_of(&raw, &context, &[]);

    let summary = report.summary().to_string();
    let setup = format!("setup head built  before cached {PREVIOUS}");
    assert!(summary.lines().any(|line| line == setup), "{summary}");
    let line = serde_json::to_string(&report.line(Utf8Path::new("run"))).unwrap();
    assert!(
        !line.contains("cached") && !line.contains("setup"),
        "{line}"
    );
}

#[test]
fn the_self_time_of_changed_files_is_the_own_cost_and_the_rest_collateral() {
    let folder = Folder::new();
    let changed_head = [
        ("src/orders.ts", "build", 10, 21, 300.0),
        ("src/orders.ts", "total", 30, 5, 20.5),
        ("src/util.ts", "format", 3, 5, 50.0),
    ];
    let changed_before = [
        ("src/orders.ts", "build", 10, 21, 100.0),
        ("src/util.ts", "format", 3, 5, 50.0),
    ];
    let mut head = profiled(&folder, "head", &changed_head);
    // A second replica adds to the repeat; a repeat without a profile has no sample.
    let log = folder.log("api-2", "", "");
    let mut replica = instance("api", Role::LongRunning, metrics(1000, 100 << 20), log);
    replica.replica = 2;
    replica.profile = Some(read(vec![sampled("src/orders.ts", "build", 10, 10.0)]));
    head[0].instances.push(replica);
    head[1].instances[0].profile = Some(ProfileOutcome::NoProfile);
    let raw = raw(
        &folder,
        head,
        Some(profiled(&folder, "before", &changed_before)),
    );
    let mut compared = context(&raw, Previous::None, &[]);
    compared.changed = Some(["src/orders.ts".to_owned()].into());
    let report = report_of(&raw, &compared, &[]);

    let owned = measure(&report, "stand.hot-owned");
    assert_eq!(owned.samples, [330.5, 320.5]);
    let before = owned.before.as_ref().expect("a before");
    assert_eq!(before.samples, [100.0, 100.0, 100.0]);
    assert_eq!((owned.unit, owned.direction), (Unit::Ms, Direction::Lower));
    assert_eq!(verdicts(owned).0, Some(Verdict::Regressed));
    let collateral = measure(&report, "stand.hot-collateral");
    assert_eq!(collateral.samples, [50.0, 50.0]);
    assert_eq!(verdicts(collateral).0, Some(Verdict::WithinNoise));
    assert_eq!(
        &names(&report)[..3],
        ["stand.wall", "stand.hot-owned", "stand.hot-collateral"]
    );

    // Neither without `--before`, nor without a profile.
    let plain = self::raw(&folder, profiled(&folder, "plain", &changed_head), None);
    let mut explored = context(&plain, Previous::None, &[]);
    explored.changed = compared.changed.clone();
    let unprofiled = self::raw(
        &folder,
        repeats(&folder, "bare", &[100]),
        Some(repeats(&folder, "bare-before", &[100])),
    );
    let mut bare = context(&unprofiled, Previous::None, &[]);
    bare.changed = compared.changed.clone();
    for report in [
        report_of(&plain, &explored, &[]),
        report_of(&unprofiled, &bare, &[]),
    ] {
        let names = names(&report);
        assert!(
            !names.iter().any(|name| name.starts_with("stand.hot")),
            "{names:?}"
        );
    }
}

// --- the grades against the declared cost ------------------------------------------------------

fn declared(measure: &str, bound: Bound) -> Declared {
    Declared {
        measure: measure.to_owned(),
        bound,
        reason: format!("the cost of {measure}"),
        post_hoc: None,
    }
}

fn accepted(measure: &str, bound: Bound, head_tree: &str, time: &str) -> Declared {
    Declared {
        post_hoc: Some(PostHoc {
            head_tree: head_tree.to_owned(),
            accepted_at: at(time),
        }),
        reason: format!("accepted {bound}"),
        ..declared(measure, bound)
    }
}

/// Three repeats of each version; `stand.wall` of "before" is 1000 ms, without noise, so the
/// threshold of the verdict is the larger of 5% and the head's spread.
fn compared_wall(head: &[u64], context: impl FnOnce(&mut Context<'_>)) -> Report {
    let folder = Folder::new();
    let before = Some(repeats(&folder, "before", &[1000, 1000, 1000]));
    let raw = raw(&folder, repeats(&folder, "head", head), before);
    let mut compared = self::context(&raw, Previous::None, &[]);
    compared.seal = Some(sealed("2026-10-22T09:40:00Z", 25));
    context(&mut compared);
    report_of(&raw, &compared, &[])
}

fn expect<'r>(report: &'r Report, name: &str) -> &'r ExpectJson {
    let expect = measure(report, name).expect.as_ref();
    expect.unwrap_or_else(|| panic!("{name} has no expect"))
}

/// The grade of `stand.wall` under `bound`, and that a fixed `api.cpu` is `met` under `none`.
fn wall_grade(head: &[u64], bound: Bound) -> Option<Grade> {
    let report = compared_wall(head, |context| {
        context.declared = vec![declared("stand.wall", bound)];
    });
    assert_eq!(expect(&report, "api.cpu").grade, Some(Grade::Met));
    expect(&report, "stand.wall").grade
}

#[test]
fn none_is_met_within_a_noise_of_5_percent_and_any_shift_is_a_surprise() {
    // +2% within a threshold of 5%; no shift under a spread of 7%; +10% that shows.
    assert_eq!(
        wall_grade(&[1020, 1020, 1020], Bound::None),
        Some(Grade::Met)
    );
    let noisy = [970, 1000, 1040];
    assert_eq!(wall_grade(&noisy, Bound::None), Some(Grade::Unmeasurable));
    assert_eq!(
        wall_grade(&[1100, 1100, 1100], Bound::None),
        Some(Grade::Surprise)
    );
    assert_eq!(
        wall_grade(&[900, 900, 900], Bound::None),
        Some(Grade::Surprise)
    );
}

#[test]
fn a_rise_up_to_x_is_met_beyond_it_exceeded_and_a_drop_a_surprise() {
    let up = Bound::UpTo(10);
    assert_eq!(wall_grade(&[1020, 1020, 1020], up), Some(Grade::Met));
    // Within a noise of 7%, which +10% can tell apart.
    assert_eq!(wall_grade(&[970, 1000, 1040], up), Some(Grade::Met));
    assert_eq!(wall_grade(&[1080, 1080, 1080], up), Some(Grade::Met));
    assert_eq!(wall_grade(&[1150, 1150, 1150], up), Some(Grade::Exceeded));
    // +15% under a threshold of 12% still shows, and beyond +10%.
    assert_eq!(wall_grade(&[1080, 1150, 1218], up), Some(Grade::Exceeded));
    assert_eq!(wall_grade(&[900, 900, 900], up), Some(Grade::Surprise));
    // No shift, but a threshold of 12% is wider than +10%.
    assert_eq!(
        wall_grade(&[940, 1000, 1060], up),
        Some(Grade::Unmeasurable)
    );
}

#[test]
fn a_drop_down_to_x_is_met_beyond_it_exceeded_and_a_rise_a_surprise() {
    let down = Bound::DownTo(10);
    assert_eq!(wall_grade(&[980, 980, 980], down), Some(Grade::Met));
    assert_eq!(wall_grade(&[970, 1000, 1040], down), Some(Grade::Met));
    assert_eq!(wall_grade(&[920, 920, 920], down), Some(Grade::Met));
    assert_eq!(wall_grade(&[850, 850, 850], down), Some(Grade::Exceeded));
    assert_eq!(wall_grade(&[782, 850, 884], down), Some(Grade::Exceeded));
    assert_eq!(wall_grade(&[1100, 1100, 1100], down), Some(Grade::Surprise));
    assert_eq!(
        wall_grade(&[940, 1000, 1060], down),
        Some(Grade::Unmeasurable)
    );
}

#[test]
fn down_is_met_by_a_drop_that_shows_on_a_neutral_measure_too() {
    assert_eq!(wall_grade(&[900, 900, 900], Bound::Down), Some(Grade::Met));
    assert_eq!(
        wall_grade(&[1100, 1100, 1100], Bound::Down),
        Some(Grade::Surprise)
    );
    // No drop shows: a surprise, unless the noise could hide one.
    assert_eq!(
        wall_grade(&[1000, 1000, 1000], Bound::Down),
        Some(Grade::Surprise)
    );
    let noisy = [940, 1000, 1060];
    assert_eq!(wall_grade(&noisy, Bound::Down), Some(Grade::Unmeasurable));

    // 8 calls to `api:3000` instead of 10: a neutral measure only changes, and `down` grades the
    // sign of the shift.
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[1000, 1000, 1000]);
    for repeat in &mut head {
        repeat.instances[1].metrics.ports[0].connections = 8;
    }
    let raw = raw(
        &folder,
        head,
        Some(repeats(&folder, "before", &[1000, 1000, 1000])),
    );
    let mut compared = context(&raw, Previous::None, &[]);
    compared.declared = vec![declared("api:3000.connections", Bound::Down)];
    let report = report_of(&raw, &compared, &[]);
    let connections = measure(&report, "api:3000.connections");
    assert_eq!(verdicts(connections).0, Some(Verdict::Changed));
    assert_eq!(
        expect(&report, "api:3000.connections").grade,
        Some(Grade::Met)
    );
}

#[test]
fn a_rise_from_zero_exceeds_any_bound_and_has_no_template() {
    let folder = Folder::new();
    let mut head = repeats(&folder, "head", &[1000, 1000, 1000]);
    for (index, repeat) in head.iter_mut().enumerate() {
        let (log, cut) = folder.log(
            &format!("head-api-{index}"),
            "ERROR: boom
",
            "",
        );
        repeat.instances[0].log = log;
        repeat.instances[0].log_cut = cut;
    }
    let raw = raw(
        &folder,
        head,
        Some(repeats(&folder, "before", &[1000, 1000, 1000])),
    );
    let mut compared = context(&raw, Previous::None, &[]);
    compared.seal = Some(sealed("2026-10-22T09:40:00Z", 1000));
    compared.declared = vec![declared("api.error-lines", Bound::UpTo(100))];
    let report = report_of(&raw, &compared, &[]);

    let errors = expect(&report, "api.error-lines");
    assert_eq!(errors.grade, Some(Grade::Exceeded));
    let next = report.line(Utf8Path::new("run")).next;
    assert!(next.is_empty(), "{next:?}");
}

#[test]
fn the_own_hot_time_of_new_code_is_a_quiet_unmeasurable_and_a_rise_from_more_grades() {
    let folder = Folder::new();
    let util = ("src/util.ts", "format", 3, 5, 50.0);
    let head = [("src/orders.ts", "build", 10, 21, 300.0), util];
    let graded = |before: &[(&str, &str, u32, u32, f64)], changed: &str, entry: Option<Bound>| {
        let raw = raw(
            &folder,
            profiled(&folder, "head", &head),
            Some(profiled(&folder, "before", before)),
        );
        let mut compared = context(&raw, Previous::None, &[]);
        compared.seal = Some(sealed("2026-10-22T09:40:00Z", 1000));
        compared.changed = Some([changed.to_owned()].into());
        compared.declared = entry
            .map(|bound| declared("stand.hot-owned", bound))
            .into_iter()
            .collect();
        report_of(&raw, &compared, &[])
    };
    let new = graded(&[util], "src/orders.ts", None);
    let owned = expect(&new, "stand.hot-owned");
    assert_eq!(
        (owned.change.as_str(), owned.reason.as_deref(), owned.grade),
        ("none", Some("new-code"), Some(Grade::Unmeasurable))
    );
    assert_eq!(expect(&new, "stand.hot-collateral").grade, Some(Grade::Met));
    let line = new.line(Utf8Path::new("run"));
    assert!(names_of(&line).is_empty(), "{:?}", names_of(&line));
    assert_eq!(line.unmeasurable, 1);
    assert!(line.next.is_empty(), "{:?}", line.next);
    // An entry changes the bound, not the grade; `unknown` still asks a person.
    let bounded = graded(&[util], "src/orders.ts", Some(Bound::UpTo(10)));
    let owned = expect(&bounded, "stand.hot-owned");
    assert_eq!(
        (owned.change.as_str(), owned.reason.as_deref(), owned.grade),
        ("+10%", Some("new-code"), Some(Grade::Unmeasurable))
    );
    assert!(names_of(&bounded.line(Utf8Path::new("run"))).is_empty());
    let asked = graded(&[util], "src/orders.ts", Some(Bound::Unknown));
    assert_eq!(expect(&asked, "stand.hot-owned").grade, None);
    assert_eq!(asked.needs.len(), 1, "{:?}", asked.needs);
    // From 100 ms the rise shows under `none`; at 0 on both sides nothing moved.
    let grown = graded(
        &[("src/orders.ts", "build", 10, 21, 100.0), util],
        "src/orders.ts",
        None,
    );
    let owned = expect(&grown, "stand.hot-owned");
    assert_eq!(
        (owned.reason.as_deref(), owned.grade),
        (None, Some(Grade::Surprise))
    );
    let line = grown.line(Utf8Path::new("run"));
    assert_eq!(names_of(&line), ["stand.hot-owned"]);
    let untouched = graded(&[util], "src/other.ts", None);
    let owned = expect(&untouched, "stand.hot-owned");
    assert_eq!(
        (owned.reason.as_deref(), owned.grade),
        (None, Some(Grade::Met))
    );
}

#[test]
fn unknown_has_no_grade_and_asks_a_person_in_needs() {
    let report = compared_wall(&[1300, 1300, 1300], |context| {
        context.declared = vec![declared("stand.wall", Bound::Unknown)];
    });

    let wall = expect(&report, "stand.wall");
    assert_eq!((wall.change.as_str(), wall.grade), ("unknown", None));
    let need = Need {
        measure: "stand.wall".to_owned(),
        reason: "the cost of stand.wall".to_owned(),
    };
    assert_eq!(report.needs, [need]);
    let line = report.line(Utf8Path::new("run"));
    assert_eq!(line.needs, report.needs);
    assert_eq!(names_of(&line), ["stand.wall"]);
    assert!(line.next.is_empty(), "{:?}", line.next);
    let json = serde_json::to_value(&line).unwrap();
    assert!(json["measures"][0]["expect"]["grade"].is_null());
    let summary = report.summary().to_string();
    let asked = "needs stand.wall: the cost of stand.wall";
    assert!(summary.lines().any(|line| line == asked), "{summary}");
}

fn names_of(line: &Line) -> Vec<&str> {
    let names = line.measures.iter();
    names.map(|measure| measure.name.as_str()).collect()
}

#[test]
fn the_accept_template_rounds_the_shift_up_to_5_percent_within_max_tolerance() {
    let accept = |bound| Next::Accept {
        measure: "stand.wall".to_owned(),
        bound,
    };
    let next = |head: &[u64], bound: Option<Bound>| {
        let report = compared_wall(head, |context| {
            context.declared = bound
                .map(|bound| declared("stand.wall", bound))
                .into_iter()
                .collect();
        });
        report.line(Utf8Path::new("run")).next
    };
    // `exceeded` at +13.2%, a `surprise` of a 7% drop, `down` within the noise.
    let exceeded = next(&[1132, 1132, 1132], Some(Bound::UpTo(10)));
    assert_eq!(exceeded, [accept(Bound::UpTo(15))]);
    assert_eq!(next(&[930, 930, 930], None), [accept(Bound::DownTo(10))]);
    let held = next(&[1000, 1000, 1000], Some(Bound::Down));
    assert_eq!(held, [accept(Bound::None)]);
    // An exact +15% stays 15, and +40% is wider than `maxTolerance` 25.
    assert_eq!(next(&[1150, 1150, 1150], None), [accept(Bound::UpTo(15))]);
    assert!(next(&[1400, 1400, 1400], None).is_empty());
    // +55% is 11.000000000000002 times 5% as a float, and still 55.
    let wide = compared_wall(&[1550, 1550, 1550], |context| {
        context.seal = Some(sealed("2026-10-22T09:40:00Z", 100));
    });
    let rounded = wide.line(Utf8Path::new("run")).next;
    assert_eq!(rounded, [accept(Bound::UpTo(55))]);
    // An `unmeasurable` entry asks for a wider bound or less noise.
    let noisy = next(&[940, 1000, 1060], Some(Bound::UpTo(10)));
    assert_eq!(noisy, [Next::Guide(Some(Topic::Expect))]);
    // Without a seal there is no `maxTolerance` to keep to.
    let report = compared_wall(&[930, 930, 930], |context| context.seal = None);
    assert!(report.line(Utf8Path::new("run")).next.is_empty());
}

#[test]
fn an_unmeasurable_measure_without_an_entry_is_quiet_and_counted() {
    let quiet = compared_wall(&[970, 1000, 1040], |_| {});
    let line = quiet.line(Utf8Path::new("run"));
    assert_eq!(
        expect(&quiet, "stand.wall").grade,
        Some(Grade::Unmeasurable)
    );
    assert_eq!(expect(&quiet, "stand.wall").reason, None);
    assert!(names_of(&line).is_empty(), "{:?}", names_of(&line));
    assert_eq!(line.unmeasurable, 1);
    assert_eq!(line.quiet as usize, quiet.measures.len());

    let declared = compared_wall(&[970, 1000, 1040], |context| {
        context.declared = vec![declared("stand.wall", Bound::None)];
    });
    let line = declared.line(Utf8Path::new("run"));
    assert_eq!(names_of(&line), ["stand.wall"]);
    assert_eq!(line.unmeasurable, 0);
    let json = serde_json::to_value(&line).unwrap();
    assert_eq!(json["unmeasurable"], 0);
}

#[test]
fn the_last_acceptance_of_this_head_replaces_the_bound_and_one_of_another_head_is_stale() {
    let report = compared_wall(&[1200, 1200, 1200], |context| {
        context.declared = vec![declared("stand.wall", Bound::UpTo(10))];
        context.accepted = vec![
            accepted(
                "stand.wall",
                Bound::UpTo(15),
                "other",
                "2026-10-22T09:50:00Z",
            ),
            accepted(
                "stand.wall",
                Bound::UpTo(25),
                HEAD_TREE,
                "2026-10-22T09:51:00Z",
            ),
            accepted(
                "stand.wall",
                Bound::UpTo(20),
                HEAD_TREE,
                "2026-10-22T09:52:00Z",
            ),
            // Another head only: `api.cpu` is `met` and still in the line.
            accepted("api.cpu", Bound::UpTo(10), "other", "2026-10-22T09:53:00Z"),
        ];
    });

    let wall = expect(&report, "stand.wall");
    let held = ExpectJson {
        change: "+20%".to_owned(),
        reason: Some("accepted +20%".to_owned()),
        grade: Some(Grade::Met),
        post_hoc: true,
        stale: false,
        acceptances: 3,
    };
    assert_eq!(wall, &held);
    let cpu = expect(&report, "api.cpu");
    assert_eq!(
        (cpu.grade, cpu.stale, cpu.post_hoc),
        (Some(Grade::Met), true, false)
    );
    assert_eq!((cpu.change.as_str(), cpu.reason.as_deref()), ("none", None));
    let line = report.line(Utf8Path::new("run"));
    assert_eq!(names_of(&line), ["stand.wall", "api.cpu"]);
    assert!(line.next.is_empty(), "{:?}", line.next);
    let json = serde_json::to_value(&line).unwrap();
    assert_eq!(json["measures"][0]["expect"]["postHoc"], true);
    assert_eq!(json["measures"][1]["expect"]["stale"], true);

    // Only the acceptance of another head: the declared bound holds, and it is exceeded.
    let report = compared_wall(&[1200, 1200, 1200], |context| {
        context.declared = vec![declared("stand.wall", Bound::UpTo(10))];
        context.accepted = vec![accepted(
            "stand.wall",
            Bound::UpTo(25),
            "other",
            "2026-10-22T09:50:00Z",
        )];
    });
    let wall = expect(&report, "stand.wall");
    assert_eq!(
        (wall.change.as_str(), wall.grade),
        ("+10%", Some(Grade::Exceeded))
    );
    assert_eq!(
        (wall.post_hoc, wall.stale, wall.acceptances),
        (false, true, 1)
    );
    let next = report.line(Utf8Path::new("run")).next;
    let template = Next::Accept {
        measure: "stand.wall".to_owned(),
        bound: Bound::UpTo(20),
    };
    assert_eq!(next, [template]);
}

#[test]
fn an_entry_of_a_measure_the_report_lacks_is_unknown_expect() {
    let typo = |context: &mut Context<'_>| {
        context.declared = vec![
            declared("stand.wall", Bound::UpTo(10)),
            declared("api.gpu", Bound::UpTo(10)),
        ];
    };
    let report = compared_wall(&[1000, 1000, 1000], typo);
    let unknown: Vec<&Flag> = report
        .flags
        .iter()
        .filter(|flag| matches!(flag, Flag::UnknownExpect { .. }))
        .collect();
    let flag = Flag::UnknownExpect {
        measure: "api.gpu".to_owned(),
    };
    assert_eq!(unknown, [&flag]);
    let json = serde_json::to_value(&flag).unwrap();
    assert_eq!(
        json,
        json!({"kind": "unknown-expect", "measure": "api.gpu"})
    );

    // A plain run grades nothing, so its entries are not looked at.
    let folder = Folder::new();
    let plain = raw_of(&folder, "plain");
    let mut explored = context(&plain, Previous::None, &[]);
    typo(&mut explored);
    let report = report_of(&plain, &explored, &[]);
    assert!(
        !report
            .flags
            .iter()
            .any(|flag| matches!(flag, Flag::UnknownExpect { .. }))
    );
    assert!(
        report
            .measures
            .iter()
            .all(|measure| measure.expect.is_none())
    );
    assert!(report.needs.is_empty());
}

#[test]
fn a_failed_before_grades_nothing() {
    let folder = Folder::new();
    let mut raw = raw(&folder, repeats(&folder, "head", &[100, 100, 100]), None);
    raw.setup.before = Some(setup_side(&folder, "before", &[]));
    raw.before = Some(BeforeOutcome::Failed(failure_of(Phase::Setup)));
    let mut compared = context(&raw, Previous::None, &[]);
    compared.declared = vec![declared("stand.wall", Bound::Unknown)];
    let report = report_of(&raw, &compared, &[]);

    assert!(
        report
            .measures
            .iter()
            .all(|measure| measure.expect.is_none())
    );
    assert!(report.needs.is_empty());
    let line = report.line(Utf8Path::new("run"));
    assert!(line.measures.is_empty() && line.next.is_empty());
}

// --- the fault repeats and the second pass -----------------------------------------------------

/// A fault repeat: the workload's code (or how it ended without one), the services that exited,
/// the workload's calls to `api:3000` with their HTTP classes, and `api`'s calls to `db:5432`.
fn faulted(
    folder: &Folder,
    repeat: u32,
    ended: Result<i32, (Phase, Cause)>,
    exited: &[&str],
    (calls, classes): (u64, [u64; 5]),
    db: u64,
) -> FaultRepeat {
    let log = || folder.log(&format!("fault-{repeat}"), "", "");
    let mut api = metrics(1000, 100 << 20);
    api.ports.push(port("db:5432", db));
    let mut load = metrics(500, 50 << 20);
    load.ports.push(PortStats {
        http: Some(classes),
        ..port("api:3000", calls)
    });
    let failure = |(phase, cause)| Failure {
        phase,
        service: Some("load".to_owned()),
        repeat: Some(repeat),
        cause,
        logs: None,
        probes: Vec::new(),
    };
    let (workload, failure) = match ended {
        Ok(0) => (Some(0), None),
        Ok(code) => (
            Some(code),
            Some(failure((Phase::Workload, Cause::Exit(code)))),
        ),
        Err(ended) => (None, Some(failure(ended))),
    };
    FaultRepeat {
        repeat,
        workload,
        exited: exited.iter().map(|service| (*service).to_owned()).collect(),
        failure,
        instances: vec![
            instance("api", Role::LongRunning, api, log()),
            instance("db", Role::LongRunning, metrics(100, 10 << 20), log()),
            instance("load", Role::Workload, load, log()),
        ],
    }
}

fn change(category: &str, before: &str, head: &str) -> CategoryChange {
    CategoryChange {
        category: category.to_owned(),
        before: before.to_owned(),
        head: head.to_owned(),
    }
}

#[test]
fn the_fault_repeats_compare_categories_and_a_disagreeing_before_is_unknown() {
    let folder = Folder::new();
    let before = Some(repeats(&folder, "before", &[100, 100, 100]));
    let mut raw = raw(&folder, repeats(&folder, "head", &[100, 100, 100]), before);
    let calm = |repeat| faulted(&folder, repeat, Ok(0), &[], (300, [0, 290, 0, 0, 10]), 2);
    let crashed =
        |repeat, ended| faulted(&folder, repeat, ended, &["api"], (40, [0, 40, 0, 0, 0]), 12);
    let timeout = Err((Phase::Workload, Cause::Timeout));
    raw.fault = Some(FaultOutcome {
        target: "db:5432".to_owned(),
        mode: "down".to_owned(),
        head: vec![crashed(1, Ok(1)), crashed(2, timeout.clone())],
        before: Some(vec![calm(1), calm(2)]),
    });
    let report = report(&raw);

    let fault = report.fault.as_ref().expect("a fault");
    let json = serde_json::to_value(fault).unwrap();
    let before = json!([
        {"category": "workload", "values": ["exit-0", "exit-0"]},
        {"category": "api state", "values": ["running", "running"]},
        {"category": "db state", "values": ["running", "running"]},
        {"category": "api:3000 attempts", "values": [">10", ">10"]},
        {"category": "db:5432 attempts", "values": ["2-3", "2-3"]},
        {"category": "api:3000 responses", "values": ["2xx 5xx", "2xx 5xx"]},
    ]);
    assert_eq!(json["categories"]["before"], before);
    assert_eq!(
        json["categories"]["head"][0]["values"],
        json!(["exit-1", "timeout"])
    );
    let changed = [
        change("workload", "exit-0", "exit-1"),
        change("workload", "exit-0", "timeout"),
        change("api state", "running", "exited"),
        change("db:5432 attempts", "2-3", ">10"),
        change("api:3000 responses", "2xx 5xx", "2xx"),
    ];
    assert_eq!(fault.changed, changed);
    assert!(!fault.unknown());
    let line = report.line(Utf8Path::new("run"));
    let shown = line.fault.as_ref().expect("a fault in the line");
    assert_eq!(
        (shown.target.as_str(), shown.mode.as_str()),
        ("db:5432", "down")
    );
    assert_eq!(shown.changed, changed);
    assert!(shown.unknown.is_empty());
    let summary = report.summary().to_string();
    for text in [
        "fault db:5432 down",
        "fault api state before running head exited",
    ] {
        assert!(
            summary.lines().any(|line| line == text),
            "{text} not in\n{summary}"
        );
    }

    // "Before" disagrees on its workload, and the head's second repeat never reached the fault.
    let fault = raw.fault.as_mut().expect("a fault");
    let exited = faulted(&folder, 2, Ok(1), &[], (300, [0, 290, 0, 0, 10]), 2);
    fault.before = Some(vec![calm(1), exited]);
    let unready = Err((Phase::Ready, Cause::Unhealthy));
    fault.head[1] = faulted(&folder, 2, unready, &["db"], (0, [0; 5]), 0);
    let report = self::report(&raw);
    let fault = report.fault.as_ref().expect("a fault");
    let head = &fault.categories.head;
    assert_eq!(head[0].values, ["exit-1", "unknown"]);
    assert_eq!(head[2].values, ["running", "unknown"], "{head:?}");
    let changed = [
        change("api state", "running", "exited"),
        change("db:5432 attempts", "2-3", ">10"),
        change("api:3000 responses", "2xx 5xx", "2xx"),
    ];
    assert_eq!(fault.changed, changed);
    assert!(fault.unknown());
    let line = report.line(Utf8Path::new("run"));
    assert_eq!(line.fault.as_ref().expect("a fault").unknown, ["workload"]);
    let summary = report.summary().to_string();
    assert!(
        summary.lines().any(|line| line == "fault workload unknown"),
        "{summary}"
    );
    // Neither repeat of "before" reached the fault: nothing compares, nothing changed.
    let mut unready = raw.fault.take().expect("a fault");
    let ready = || {
        faulted(
            &folder,
            1,
            Err((Phase::Ready, Cause::Unhealthy)),
            &[],
            (0, [0; 5]),
            0,
        )
    };
    unready.before = Some(vec![ready(), ready()]);
    raw.fault = Some(unready);
    let blind = self::report(&raw);
    let fault_of = blind.fault.as_ref().expect("a fault");
    assert!(fault_of.changed.is_empty(), "{:?}", fault_of.changed);
    let unknown = blind
        .line(Utf8Path::new("run"))
        .fault
        .expect("a fault")
        .unknown;
    assert_eq!(unknown.len(), fault_of.categories.head.len());
    // A loaded report tells it the same: `cli` reads it there for the pool.
    let loaded: FaultJson = serde_json::from_value(serde_json::to_value(fault).unwrap()).unwrap();
    assert!(loaded.unknown());

    // Without "before" nothing is compared, and only a head that never reached the fault is unknown.
    let mut alone = raw_of(&folder, "alone");
    alone.fault = Some(FaultOutcome {
        target: "db:5432".to_owned(),
        mode: "slow 2000ms".to_owned(),
        head: vec![calm(1), calm(2)],
        before: None,
    });
    let fault = self::report(&alone).fault.expect("a fault");
    assert!(fault.categories.before.is_none() && fault.changed.is_empty());
    assert!(!fault.unknown());
    let unready = || {
        faulted(
            &folder,
            1,
            Err((Phase::Ready, Cause::Unhealthy)),
            &[],
            (0, [0; 5]),
            0,
        )
    };
    alone.fault.as_mut().expect("a fault").head = vec![unready(), unready()];
    assert!(self::report(&alone).fault.expect("a fault").unknown());
}

/// `pass2-<side>/out-<n>` with `files`, and the open connections by target after the pass.
fn pass(folder: &Folder, side: &str, n: u32, files: &[(&str, &str)], open: &[(&str, u64)]) -> Pass {
    let dir = folder
        .path()
        .join(format!("pass2-{side}"))
        .join(format!("out-{n}"));
    for (file, text) in files {
        let path = dir.join(file);
        fs::create_dir_all(path.parent().expect("a parent")).expect("create a pass folder");
        fs::write(path, text).expect("write a pass output");
    }
    fs::create_dir_all(&dir).expect("create a pass folder");
    let ports = open.iter().map(|(target, open)| PortStats {
        open: *open,
        ..port(target, 1)
    });
    Pass {
        out_dir: dir,
        probes: Vec::new(),
        ports: ports.collect(),
    }
}

fn passed(first: Pass, second: Pass) -> PassPair {
    PassPair::Passed { first, second }
}

fn pass_file(report: &Report, file: &str) -> PassFile {
    let pass = report.second_pass.as_ref().expect("a second pass");
    let found = pass.files.iter().find(|found| found.file == file);
    found.unwrap_or_else(|| panic!("no file {file}")).clone()
}

#[test]
fn an_unchanged_file_that_changed_on_the_head_is_in_the_second_pass_and_open_connections_grow() {
    let folder = Folder::new();
    // `at` changes from repeat to repeat on both versions: noise, not a change of the state.
    let measured = |side: &str, at: [u32; 2]| {
        let files = at.map(|at| format!("{{\"at\":{at},\"total\":10}}\n"));
        let files: [&[(&str, &str)]; 2] = [
            &[("totals.jsonl", files[0].as_str())],
            &[("totals.jsonl", files[1].as_str())],
        ];
        with_files(&folder, side, &files)
    };
    let mut raw = raw(
        &folder,
        measured("head", [1, 2]),
        Some(measured("before", [3, 4])),
    );
    probe_rule(&mut raw, "orders", Some("/id"), &["/updated_at"]);
    let one = "{\"id\":1,\"status\":\"paid\",\"updated_at\":1}\n";
    let two = format!("{one}{{\"id\":2,\"status\":\"paid\",\"updated_at\":2}}\n");
    let head = passed(
        pass(
            &folder,
            "head",
            1,
            &[
                ("probe.orders.jsonl", one),
                ("totals.jsonl", "{\"at\":5,\"total\":10}\n"),
                ("sub/state.jsonl", "{\"n\":1}\n"),
            ],
            &[("db:5432", 2), ("api:3000", 0)],
        ),
        pass(
            &folder,
            "head",
            2,
            &[
                ("probe.orders.jsonl", two.as_str()),
                ("totals.jsonl", "{\"at\":6,\"total\":10}\n"),
                ("sub/state.jsonl", "{\"n\":2}\n"),
            ],
            &[("db:5432", 4), ("api:3000", 0)],
        ),
    );
    let before = passed(
        pass(
            &folder,
            "before",
            1,
            &[
                ("probe.orders.jsonl", one),
                ("totals.jsonl", "{\"at\":7,\"total\":10}\n"),
                ("sub/state.jsonl", "{\"n\":1}\n"),
            ],
            &[("db:5432", 2)],
        ),
        pass(
            &folder,
            "before",
            2,
            &[
                ("probe.orders.jsonl", one),
                ("totals.jsonl", "{\"at\":8,\"total\":20}\n"),
                ("sub/state.jsonl", "{\"n\":2}\n"),
            ],
            &[("db:5432", 2)],
        ),
    );
    let unchanged = ["probe.orders.jsonl", "totals.jsonl", "sub/state.jsonl"];
    raw.second_pass = Some(SecondPassOutcome {
        unchanged: unchanged.map(str::to_owned).to_vec(),
        head,
        before: Some(before),
    });
    let context = context(&raw, Previous::None, &[]);
    let (report, dir) = written(&folder, "run", &raw, &context);

    let orders = PassFile {
        file: "probe.orders.jsonl".to_owned(),
        before: Some(PassState::Same),
        head: PassState::Changed,
        diffs: vec![Diff {
            at: At::Key { key: json!(2) },
            path: String::new(),
            before: Value::Null,
            head: json!({"id": 2, "status": "paid", "updated_at": 2}),
        }],
    };
    assert_eq!(pass_file(&report, "probe.orders.jsonl"), orders);
    let totals = pass_file(&report, "totals.jsonl");
    assert_eq!(
        (totals.before, totals.head),
        (Some(PassState::Changed), PassState::Same)
    );
    let state = pass_file(&report, "sub/state.jsonl");
    assert_eq!(
        (state.before, state.head),
        (Some(PassState::Changed), PassState::Changed)
    );
    let open = serde_json::to_value(&report.second_pass.as_ref().expect("a pass").open).unwrap();
    let expected = json!([
        {"target": "api:3000", "side": "head", "first": 0, "second": 0},
        {"target": "db:5432", "side": "head", "first": 2, "second": 4},
        {"target": "db:5432", "side": "before", "first": 2, "second": 2},
    ]);
    assert_eq!(open, expected);
    let grown: Vec<&Flag> = report
        .flags
        .iter()
        .filter(|flag| matches!(flag, Flag::ConnectionsGrow { .. }))
        .collect();
    let grow = Flag::ConnectionsGrow {
        target: "db:5432".to_owned(),
        side: "head".to_owned(),
        first: 2,
        second: 4,
    };
    assert_eq!(grown, [&grow]);

    let line = report.line(&dir);
    let passes = line
        .second_pass
        .as_ref()
        .expect("a second pass in the line");
    let files: Vec<(&str, Option<PassState>, u32)> = passes
        .iter()
        .map(|file| (file.file.as_str(), file.before, file.diffs))
        .collect();
    let shown = [
        ("probe.orders.jsonl", Some(PassState::Same), 1),
        ("sub/state.jsonl", Some(PassState::Changed), 1),
    ];
    assert_eq!(files, shown);
    let pass2 = dir.join("diff").join("pass2");
    assert_eq!(passes[0].path, pass2.join("probe.orders.jsonl.json"));
    assert_eq!(passes[0].sample, orders.diffs);
    // `totals.jsonl` held on the head.
    let quiet = report.measures.len() + report.behaviour.as_ref().map_or(0, Vec::len) + 1;
    assert_eq!(
        line.quiet as usize,
        quiet - line.measures.len() - line_files(&line).len()
    );
    let json = serde_json::to_value(&line).unwrap();
    assert_eq!(
        json["secondPass"][1]["path"],
        format!("{}/sub/state.jsonl.json", slashed(&pass2))
    );
    let read = |file: &str| -> Value {
        let text = fs::read_to_string(pass2.join(file)).expect("a diff of the second pass");
        serde_json::from_str(&text).expect("JSON")
    };
    assert_eq!(
        read("sub/state.jsonl.json"),
        json!([{"record": 1, "path": "/n", "before": 1, "head": 2}])
    );
    assert_eq!(read("totals.jsonl.json"), json!([]));
    let loaded = load(&dir).expect("the report loads");
    assert_eq!(loaded.second_pass, report.second_pass);
    let summary = report.summary().to_string();
    let text = "secondPass probe.orders.jsonl before same diffs 1";
    assert!(summary.lines().any(|line| line == text), "{summary}");
}

#[test]
fn a_failed_second_pass_is_a_flag_and_a_failed_head_has_no_second_pass() {
    let folder = Folder::new();
    let before = Some(repeats(&folder, "before", &[100]));
    let mut raw = raw(&folder, repeats(&folder, "head", &[100]), before);
    let file = [("state.jsonl", "{\"n\":1}\n")];
    let failed = |phase| PassPair::Failed(failure_of(phase));
    raw.second_pass = Some(SecondPassOutcome {
        unchanged: vec!["state.jsonl".to_owned()],
        head: passed(
            pass(&folder, "head", 1, &file, &[]),
            pass(&folder, "head", 2, &file, &[]),
        ),
        before: Some(failed(Phase::Ready)),
    });
    let report = report(&raw);

    let state = pass_file(&report, "state.jsonl");
    assert_eq!((state.before, state.head), (None, PassState::Same));
    let flag = Flag::SecondPassFailed {
        side: "before".to_owned(),
        stage: Phase::Ready,
    };
    assert!(report.flags.contains(&flag), "{:?}", report.flags);
    let json = serde_json::to_value(&flag).unwrap();
    assert_eq!(
        json,
        json!({"kind": "second-pass-failed", "side": "before", "stage": "ready"})
    );
    let line = report.line(Utf8Path::new("run"));
    assert_eq!(line.second_pass.as_ref().map(Vec::len), Some(0));

    let second = raw.second_pass.as_mut().expect("a second pass");
    second.head = failed(Phase::Workload);
    let report = self::report(&raw);
    assert!(report.second_pass.is_none());
    let flags = report.flags.iter();
    let failed: Vec<&Flag> = flags
        .filter(|flag| matches!(flag, Flag::SecondPassFailed { .. }))
        .collect();
    let head = Flag::SecondPassFailed {
        side: "head".to_owned(),
        stage: Phase::Workload,
    };
    assert_eq!(failed, [&head, &flag]);
    assert!(report.line(Utf8Path::new("run")).second_pass.is_none());
}

#[test]
fn a_second_pass_file_reads_only_through_plain_folders() {
    let folder = Folder::new();
    let mut raw = raw(&folder, repeats(&folder, "head", &[100]), None);
    let first = pass(&folder, "head", 1, &[("sub", "a file, not a folder")], &[]);
    let second = pass(&folder, "head", 2, &[], &[]);
    raw.second_pass = Some(SecondPassOutcome {
        unchanged: vec!["sub/state.jsonl".to_owned()],
        head: passed(first, second),
        before: None,
    });
    let refused = read_outputs(&raw, &[]);
    assert!(
        matches!(refused, Err(ReportError::Bad { .. })),
        "{refused:?}"
    );

    // A pass that wrote no such file has no records of it.
    let second = raw.second_pass.as_mut().expect("a second pass");
    second.unchanged = vec!["missing.jsonl".to_owned()];
    let report = self::report(&raw);
    let missing = pass_file(&report, "missing.jsonl");
    assert_eq!((missing.before, missing.head), (None, PassState::Same));
}

#[test]
fn a_pooled_rerun_takes_coverage_fault_and_second_pass_from_the_last_run_with_from() {
    let folder = Folder::new();
    // The first run makes all three: the fixture's run, with its fault and second pass.
    let mut first = fixture_raw(&folder, &[1000, 1000, 1000], Some(&[1000, 1000, 1000]));
    fixture_modes(&folder, &mut first);
    let mut made = context(&first, Previous::None, &[]);
    made.run = "20261022-090000".to_owned();
    let (made, _) = written(&folder, "made", &first, &made);
    for from in [
        made.coverage.as_ref().map(|coverage| &coverage.from),
        made.fault.as_ref().map(|fault| &fault.from),
        made.second_pass.as_ref().map(|pass| &pass.from),
    ] {
        assert_eq!(from, Some(&None), "a result of its own has no `from`");
    }

    // A rerun of the same trees ran none of them: `run` left them out of `Raw`.
    let mut rerun = fixture_raw(&folder, &[1000, 1000, 1000], Some(&[1000, 1000, 1000]));
    rerun.coverage = None;
    let all = BTreeSet::from([Extra::Coverage, Extra::Fault, Extra::SecondPass]);
    let pool = [made.clone()];
    let mut taken = context(&rerun, Previous::None, &pool);
    taken.run = "20261022-091000".to_owned();
    taken.reused = all.clone();
    let (copy, dir) = written(&folder, "copy", &rerun, &taken);

    let from = Some("20261022-090000".to_owned());
    let coverage = copy.coverage.as_ref().expect("the coverage of the pool");
    assert_eq!(coverage.from, from);
    assert_eq!(
        serde_json::to_value(&coverage.called).unwrap(),
        serde_json::to_value(&made.coverage.as_ref().unwrap().called).unwrap()
    );
    let fault = copy.fault.as_ref().expect("the fault of the pool");
    assert_eq!(fault.from, from);
    assert_eq!(fault.changed, made.fault.as_ref().unwrap().changed);
    let pass = copy
        .second_pass
        .as_ref()
        .expect("the second pass of the pool");
    assert_eq!(pass.from, from);
    assert_eq!(pass.files, made.second_pass.as_ref().unwrap().files);
    // The copied open connections still grow, and `write` put the copied diffs in this run.
    assert!(
        copy.flags
            .iter()
            .any(|flag| matches!(flag, Flag::ConnectionsGrow { .. }))
    );
    let pass2 = dir
        .join("diff")
        .join("pass2")
        .join("probe.orders.jsonl.json");
    let written: Value =
        serde_json::from_str(&fs::read_to_string(&pass2).expect("a copied diff")).unwrap();
    assert_eq!(written, serde_json::to_value(&pass.files[0].diffs).unwrap());
    let line = copy.line(&dir);
    assert_eq!(line.fault.as_ref().expect("a fault").from, from);
    let passes = line.second_pass.as_ref().expect("a second pass");
    assert!(!passes.is_empty());
    for file in passes {
        assert_eq!(file.from, from);
        assert!(file.path.starts_with(&dir), "{}", file.path);
    }
    let summary = copy.summary().to_string();
    let text = "fault db:5432 down from 20261022-090000";
    assert!(summary.lines().any(|line| line == text), "{summary}");

    // A copy of the copy keeps where the result came from.
    let pool = [made, copy];
    let mut again = context(&rerun, Previous::None, &pool);
    again.run = "20261022-092000".to_owned();
    again.reused = all;
    let twice = report_of(&rerun, &again, &[]);
    assert_eq!(twice.coverage.expect("a coverage").from, from);
    assert_eq!(twice.fault.expect("a fault").from, from);
    assert_eq!(twice.second_pass.expect("a second pass").from, from);

    // The last run of the pool gives the result, not an older one.
    let [made, copy] = pool;
    let mut fresh = made;
    fresh.run = "20261022-093000".to_owned();
    let pool = [copy, fresh];
    let mut latest = context(&rerun, Previous::None, &pool);
    latest.reused = BTreeSet::from([Extra::Fault]);
    let fault = report_of(&rerun, &latest, &[]).fault.expect("a fault");
    assert_eq!(fault.from.as_deref(), Some("20261022-093000"));

    // Without `reused` the run's own results count, and it has none here.
    let own = report_of(&rerun, &context(&rerun, Previous::None, &pool), &[]);
    assert!(own.coverage.is_none() && own.fault.is_none() && own.second_pass.is_none());
}

// --- the seal, the attempts and the comparisons -----------------------------------------------

fn past(run: &str, time: &str, head_tree: &str, compared: bool, passed: bool) -> PastRun {
    PastRun {
        run: run.to_owned(),
        created_at: at(time),
        head_tree: head_tree.to_owned(),
        compared,
        passed,
    }
}

fn sealed(time: &str, max_tolerance: u16) -> SealInfo {
    SealInfo {
        sealed_at: at(time),
        max_tolerance,
        journal: vec![Journal::Sealed {
            sealed_at: at(time),
            digest: DIGEST.to_owned(),
        }],
    }
}

fn seal_flags(report: &Report) -> Vec<&Flag> {
    let flags = report.flags.iter().filter(|flag| {
        matches!(
            flag,
            Flag::BeforeAfterSession { .. }
                | Flag::MeasuredBeforeSeal { .. }
                | Flag::WideTolerance { .. }
        )
    });
    flags.collect()
}

fn failure_of(phase: Phase) -> Failure {
    Failure {
        phase,
        service: None,
        repeat: None,
        cause: Cause::Timeout,
        logs: None,
        probes: Vec::new(),
    }
}

#[test]
fn the_seal_its_journal_and_the_comparisons_since_it_are_in_the_report_and_the_line() {
    let folder = Folder::new();
    let before = Some(repeats(&folder, "before", &[100]));
    let raw = raw(&folder, repeats(&folder, "head", &[100]), before);
    let mut compared = context(&raw, Previous::None, &[]);
    let old = "0".repeat(64);
    compared.seal = Some(SealInfo {
        journal: vec![
            Journal::Sealed {
                sealed_at: at("2026-10-22T08:00:00Z"),
                digest: old.clone(),
            },
            Journal::Cleaned {
                cleaned_at: at("2026-10-22T08:30:00Z"),
            },
            Journal::Resealed {
                resealed_at: at("2026-10-22T09:40:00Z"),
                digest: DIGEST.to_owned(),
                reason: "a new probe".to_owned(),
            },
        ],
        ..sealed("2026-10-22T09:40:00Z", 25)
    });
    // Only the passed runs with `--before` since the reseal count: not one before it, a failed
    // one or a plain one.
    compared.history = vec![
        past(
            "20261022-093000",
            "2026-10-22T09:30:00Z",
            "other",
            true,
            true,
        ),
        past(
            "20261022-094100",
            "2026-10-22T09:41:00Z",
            "other",
            true,
            false,
        ),
        past(
            "20261022-094200",
            "2026-10-22T09:42:00Z",
            "other",
            false,
            true,
        ),
        past(
            "20261022-094500",
            "2026-10-22T09:45:00Z",
            "other",
            true,
            true,
        ),
    ];
    let report = report_of(&raw, &compared, &[]);

    let json = serde_json::to_value(&report).unwrap();
    let seal = json!({
        "sealedAt": "2026-10-22T09:40:00Z",
        "maxTolerance": 25,
        "journal": [
            {"sealedAt": "2026-10-22T08:00:00Z", "digest": old},
            {"cleanedAt": "2026-10-22T08:30:00Z"},
            {"resealedAt": "2026-10-22T09:40:00Z", "digest": DIGEST, "reason": "a new probe"},
        ],
    });
    assert_eq!(json["seal"], seal);
    assert_eq!(
        (&json["attempts"], &json["comparisons"]),
        (&json!(1), &json!(2))
    );
    let back: Report = serde_json::from_value(json).expect("the report reads back");
    assert_eq!(back.seal, report.seal);
    assert!(seal_flags(&report).is_empty(), "{:?}", report.flags);
    let line = report.line(Utf8Path::new("run"));
    assert_eq!(line.seal, report.seal);
    assert_eq!((line.attempts, line.comparisons), (1, Some(2)));
    let summary = report.summary().to_string();
    let reseal =
        format!("journal digest {DIGEST} reason a new probe resealedAt 2026-10-22T09:40:00Z");
    let printed = [
        "seal 2026-10-22T09:40:00Z  maxTolerance 25",
        "journal cleanedAt 2026-10-22T08:30:00Z",
        reseal.as_str(),
        "attempts 1  comparisons 2",
    ];
    for text in printed {
        assert!(
            summary.lines().any(|line| line == text),
            "{text} not in\n{summary}"
        );
    }

    // Without `--before` neither is there, whatever the context holds.
    let plain = raw_of(&folder, "plain");
    let mut explored = context(&plain, Previous::None, &[]);
    explored.seal = Some(sealed("2026-10-22T09:40:00Z", 40));
    explored.history = compared.history.clone();
    let report = report_of(&plain, &explored, &[]);
    let json = serde_json::to_value(&report).unwrap();
    assert!(
        json["seal"].is_null() && json["comparisons"].is_null(),
        "{json}"
    );
    assert!(seal_flags(&report).is_empty(), "{:?}", report.flags);
    let line = Line::failed(failure_of(Phase::Workload), Some(&plain), Vec::new());
    assert!(line.seal.is_none() && line.attempts == 0 && line.comparisons.is_none());
}

#[test]
fn a_wide_tolerance_a_run_before_the_seal_and_a_late_before_are_flags() {
    let folder = Folder::new();
    let before = Some(repeats(&folder, "before", &[100]));
    let raw = raw(&folder, repeats(&folder, "head", &[100]), before);
    let mut compared = context(&raw, Previous::None, &[]);
    compared.seal = Some(sealed("2026-10-22T09:40:00Z", 40));
    // "Before" is from 09:12 and the session began at 09:10 with a run of this head. The failed
    // run, the run of another head and the one after the seal measured nothing before it.
    compared.history = vec![
        past(
            "20261022-091000",
            "2026-10-22T09:10:00Z",
            HEAD_TREE,
            false,
            true,
        ),
        past(
            "20261022-092000",
            "2026-10-22T09:20:00Z",
            HEAD_TREE,
            false,
            true,
        ),
        past(
            "20261022-093000",
            "2026-10-22T09:30:00Z",
            HEAD_TREE,
            false,
            false,
        ),
        past(
            "20261022-093500",
            "2026-10-22T09:35:00Z",
            "other",
            false,
            true,
        ),
        past(
            "20261022-094500",
            "2026-10-22T09:45:00Z",
            HEAD_TREE,
            true,
            true,
        ),
    ];
    let report = report_of(&raw, &compared, &[]);

    let expected = [
        Flag::BeforeAfterSession {
            taken_at: "2026-10-22T09:12:00Z".to_owned(),
            started_at: "2026-10-22T09:10:00Z".to_owned(),
        },
        Flag::MeasuredBeforeSeal {
            run: "20261022-092000".to_owned(),
        },
        Flag::WideTolerance { max_tolerance: 40 },
    ];
    assert_eq!(seal_flags(&report), expected.iter().collect::<Vec<_>>());
    let late = json!({
        "kind": "before-after-session",
        "takenAt": "2026-10-22T09:12:00Z",
        "startedAt": "2026-10-22T09:10:00Z",
    });
    assert_eq!(serde_json::to_value(&expected[0]).unwrap(), late);
    let wide = json!({"kind": "wide-tolerance", "maxTolerance": 40});
    assert_eq!(serde_json::to_value(&expected[2]).unwrap(), wide);

    // At 25, with "before" taken first and no run of this head before the seal: none.
    compared.seal = Some(sealed("2026-10-22T09:40:00Z", 25));
    compared.history = vec![past(
        "20261022-091500",
        "2026-10-22T09:15:00Z",
        "other",
        false,
        true,
    )];
    assert!(seal_flags(&report_of(&raw, &compared, &[])).is_empty());
    // With no earlier run the seal starts the session.
    compared.seal = Some(sealed("2026-10-22T09:00:00Z", 25));
    compared.history.clear();
    let report = report_of(&raw, &compared, &[]);
    let started = Flag::BeforeAfterSession {
        taken_at: "2026-10-22T09:12:00Z".to_owned(),
        started_at: "2026-10-22T09:00:00Z".to_owned(),
    };
    assert_eq!(seal_flags(&report), [&started]);
}

#[test]
fn attempts_count_the_pooled_runs_and_an_unreadable_report_is_left_out() {
    let folder = Folder::new();
    let dirs: Vec<Utf8PathBuf> = ["a", "b", "c"]
        .iter()
        .map(|name| {
            let raw = raw_of(&folder, name);
            let context = context(&raw, Previous::None, &[]);
            written(&folder, name, &raw, &context).1
        })
        .collect();
    fs::write(dirs[1].join("report.json"), "{").expect("break a pooled report");
    // As `cli` pools: the reports that load.
    let pool: Vec<Report> = dirs.iter().filter_map(|dir| load(dir).ok()).collect();
    let raw = raw_of(&folder, "head");
    let report = report_of(&raw, &context(&raw, Previous::None, &pool), &[]);

    assert_eq!(report.attempts, 3);
    assert_eq!(report.line(Utf8Path::new("run")).attempts, 3);
    assert_eq!(self::report(&raw).attempts, 1);
}

// --- the fixture -------------------------------------------------------------------------------

/// The run of the fixture: `db` with the volume `pgdata` and the probe `orders`, `api` profiled,
/// the workload `load` calling `api:3000`; three repeats per version, `--before session`.
fn fixture_repeat(folder: &Folder, side: &str, repeat: u32, wall: u64) -> RepeatOutput {
    let dir = format!("fixture-{side}-{repeat}");
    let head = side == "head";
    let (second, order) = if head { (11, "pending") } else { (10, "paid") };
    let id = &side[..1];
    let record = format!(
        "{{\"order\":1,\"requestId\":\"{id}{repeat}a\",\"total\":10}}\n{{\"order\":2,\"requestId\":\"{id}{repeat}b\",\"total\":{second}}}\n"
    );
    let orders = format!(
        "{{\"id\":7,\"status\":\"{order}\",\"updated_at\":\"{side}-{repeat}\"}}\n{{\"id\":8,\"status\":\"paid\",\"updated_at\":\"{side}-{repeat}\"}}\n"
    );
    let files = [
        ("out.jsonl", record.as_str()),
        ("probe.orders.jsonl", orders.as_str()),
        ("probe.orders.jsonl.err", ""),
    ];
    let out = folder.out(&dir, &files);

    let mut api = metrics(if head { 6000 } else { 5000 }, 300 << 20);
    api.ports.push(PortStats {
        bytes_out: 20_000,
        bytes_in: 80_000,
        ..port("db:5432", 4)
    });
    let api_log = folder.log(
        &format!("{dir}-api"),
        "listening on 3000\nERROR: boom\n",
        "ERROR: late\n",
    );
    let mut api = instance("api", Role::LongRunning, api, api_log);
    api.stopped = Some(Stopped {
        after: Duration::from_millis(120),
        killed: false,
    });
    let (build_line, build_self, money_self) = if head {
        (40, 812.0, 120.0)
    } else {
        (38, 500.0, 118.0)
    };
    let mut functions = vec![
        Sampled {
            total_ms: 2100.0,
            column: 21,
            ..sampled(
                "src/orders/orders.service.ts",
                "buildReport",
                build_line,
                build_self,
            )
        },
        Sampled {
            total_ms: money_self + 10.0,
            column: 28,
            ..sampled("src/util/money.ts", "formatMoney", 3, money_self)
        },
    ];
    // Under 1 ms: left out of `hot`.
    if head {
        functions.push(Sampled {
            column: 5,
            ..sampled("src/util/money.ts", "round", 10, 0.5)
        });
    }
    api.profile = Some(read(functions));

    let db_log = folder.log(&format!("{dir}-db"), "ready\n", "");
    let mut db = instance("db", Role::LongRunning, metrics(3000, 200 << 20), db_log);
    db.stopped = Some(Stopped {
        after: Duration::from_millis(300),
        killed: false,
    });

    let mut load = metrics(1000, 50 << 20);
    load.memory_limit = 512 << 20;
    load.ports.push(PortStats {
        bytes_out: 150_000,
        bytes_in: 600_000,
        http: Some([0, 3000, 0, 0, 0]),
        drops: if head && repeat == 2 { 14 } else { 0 },
        ..port("api:3000", 300)
    });
    let load = instance(
        "load",
        Role::Workload,
        load,
        folder.log(&format!("{dir}-load"), "", ""),
    );

    RepeatOutput {
        repeat,
        wall: Duration::from_millis(wall),
        instances: vec![api, db, load],
        probes: vec![probe_ran("orders", ExecOutcome::Exited(0), &out)],
        out_dir: out,
    }
}

fn fixture_raw(folder: &Folder, head_walls: &[u64], before_walls: Option<&[u64]>) -> Raw {
    let side = |name: &str, walls: &[u64]| -> Vec<RepeatOutput> {
        let numbered = walls.iter().zip(1..);
        numbered
            .map(|(wall, repeat)| fixture_repeat(folder, name, repeat, *wall))
            .collect()
    };
    let mut raw = raw(
        folder,
        side("head", head_walls),
        before_walls.map(|walls| side("before", walls)),
    );
    raw.setup.head.sizes.insert("pgdata".to_owned(), 8_388_608);
    if let Some(setup) = &mut raw.setup.before {
        setup.sizes.insert("pgdata".to_owned(), 8_126_464);
    }
    probe_rule(&mut raw, "orders", Some("/id"), &["/updated_at"]);
    let coverage = Coverage {
        called: vec![
            called("src/orders/orders.service.ts", 40, 21, 1200),
            called("src/util/money.ts", 3, 28, 5000),
        ],
    };
    let services = BTreeMap::from([("api".to_owned(), coverage)]);
    raw.coverage = Some(CoverageOutcome::Read(services));
    raw
}

/// With `db:5432` down, `api` of the head dies and the workload exits 1, where "before" answered
/// 503 and finished; the second pass of the head adds order 9 and leaves two more connections to
/// `db` open.
fn fixture_modes(folder: &Folder, raw: &mut Raw) {
    let calm = |repeat| faulted(folder, repeat, Ok(0), &[], (300, [0, 280, 0, 0, 20]), 2);
    let crashed = |repeat| faulted(folder, repeat, Ok(1), &["api"], (300, [0, 150, 0, 0, 0]), 2);
    raw.fault = Some(FaultOutcome {
        target: "db:5432".to_owned(),
        mode: "down".to_owned(),
        head: vec![crashed(1), crashed(2)],
        before: Some(vec![calm(1), calm(2)]),
    });
    let orders = |side: &str, n: u32, statuses: &[(u32, &str)]| -> String {
        let records = statuses.iter().map(|(id, status)| {
            format!(
                "{{\"id\":{id},\"status\":\"{status}\",\"updated_at\":\"{side}-pass-{n}\"}}
"
            )
        });
        records.collect()
    };
    let side = |side: &str, statuses: [&[(u32, &str)]; 2], open: [u64; 2]| {
        let [first, second] = [1, 2].map(|n| {
            let text = orders(side, n, statuses[n as usize - 1]);
            let open = [("db:5432", open[n as usize - 1])];
            pass(
                folder,
                side,
                n,
                &[("probe.orders.jsonl", text.as_str())],
                &open,
            )
        });
        passed(first, second)
    };
    let head: &[(u32, &str)] = &[(7, "pending"), (8, "paid")];
    let more: &[(u32, &str)] = &[(7, "pending"), (8, "paid"), (9, "pending")];
    let paid: &[(u32, &str)] = &[(7, "paid"), (8, "paid")];
    raw.second_pass = Some(SecondPassOutcome {
        unchanged: vec!["probe.orders.jsonl".to_owned()],
        head: side("head", [head, more], [2, 4]),
        before: Some(side("before", [paid, paid], [2, 2])),
    });
}

#[test]
fn make_on_the_raw_outputs_and_context_of_the_fixture_gives_the_fixture() {
    let folder = Folder::new();
    let earlier = fixture_raw(&folder, &[41900, 41800, 42000], None);
    let mut earlier_context = context(&earlier, Previous::None, &[]);
    earlier_context.run = PREVIOUS.to_owned();
    earlier_context.created_at = at("2026-10-22T09:45:00Z");
    let previous = report_of(&earlier, &earlier_context, &[]);
    let mut raw = fixture_raw(
        &folder,
        &[42210, 41877, 42502],
        Some(&[41010, 40870, 41502]),
    );
    fixture_modes(&folder, &mut raw);
    let mut context = context(&raw, Previous::Read(&previous), &[]);
    context.setup_from.before = Some(PREVIOUS.to_owned());
    context.changed = Some(["src/orders/orders.service.ts".to_owned()].into());
    // An early plain run of another head, then the previous run, the first comparison.
    context.seal = Some(sealed("2026-10-22T09:40:00Z", 25));
    context.history = vec![
        past(
            "20261022-091500",
            "2026-10-22T09:15:00Z",
            "other",
            false,
            true,
        ),
        past(PREVIOUS, "2026-10-22T09:45:00Z", "other", true, true),
    ];
    let reason = |entry: Declared, reason: &str| Declared {
        reason: reason.to_owned(),
        ..entry
    };
    context.declared = vec![
        reason(
            declared("stand.wall", Bound::UpTo(10)),
            "one more index lookup per order",
        ),
        reason(declared("db.cpu", Bound::Unknown), "a new query plan"),
    ];
    let cpu = accepted(
        "api.cpu",
        Bound::UpTo(20),
        HEAD_TREE,
        "2026-10-22T09:50:00Z",
    );
    context.accepted = vec![reason(cpu, "the report formats every order")];
    let report = report_of(&raw, &context, &[]);

    let expected: Value = serde_json::from_str(FIXTURE).expect("the fixture is JSON");
    let made = serde_json::to_value(&report).expect("the report serializes");
    assert_eq!(
        made,
        expected,
        "made:\n{}",
        serde_json::to_string_pretty(&made).unwrap()
    );
}
