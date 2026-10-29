//! `read_outputs` and `make` on values of `Raw` and `Context` (blueprint, Tests `report/`). No
//! VM: a temporary folder stands in for the run folder that `run` leaves.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::time::{Duration, SystemTime};

use camino::{Utf8Path, Utf8PathBuf};
use profile::{Called, Coverage, Sampled, Samples};
use report::{
    BeforeInfo, BeforeOutcome, Behaviour, Bound, Cause, Context, CoverageOutcome, Declared,
    Failure, FaultOutcome, FaultRepeat, InstanceOutput, Intent, Journal, Line, Measure, Pass,
    PassFile, PassPair, PastRun, Phase, PostHoc, Previous, ProbeRan, ProbeRule, ProfileOutcome,
    Raw, RepeatOutput, Report, Role, SealInfo, SecondPassOutcome, Setup, SetupFrom, SetupSide,
    Verdict, make, read_outputs,
};
use sandbox::{ExecOutcome, Metrics, PortStats, Stopped};
use tempfile::TempDir;

mod behaviour;
mod fixture;
mod flags;
mod grades;
mod hot;
mod line;
mod measures;
mod modes;
mod seal;

const GIB: u64 = 1 << 30;
const RUN: &str = "20261022-100000";
const PREVIOUS: &str = "20261022-094500";
const HEAD_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const BEFORE_TREEISH: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";
const BEFORE_TREE: &str = "7d1e09f3c2b4a5968778695a4b3c2d1e0f9a8b7c";
const DIGEST: &str = "9f2c41aa6b0e4d3c8a7f5e1d2c4b6a8f0e9d7c5b3a1f2e4d6c8b0a9f7e5d3c1b";
const FIXTURE: &str = include_str!("../../../../../shared/fixtures/report.json");

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

fn raw_of(folder: &Folder, name: &str) -> Raw {
    raw(folder, repeats(folder, name, &[100]), None)
}

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

fn probe_rule(raw: &mut Raw, probe: &str, key: Option<&str>, ignore: &[&str]) {
    let rule = ProbeRule {
        key: key.map(str::to_owned),
        ignore: ignore.iter().map(|path| (*path).to_owned()).collect(),
    };
    raw.probes.insert(format!("probe.{probe}.jsonl"), rule);
}

fn line_files(line: &Line) -> Vec<(&str, u32, usize)> {
    let files = line.behaviour.iter().flatten();
    let files = files.map(|file| (file.file.as_str(), file.diffs, file.sample.len()));
    files.collect()
}

/// A path as the report and the line print it.
fn slashed(path: &Utf8Path) -> String {
    path.as_str().replace('\\', "/")
}

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
        service: Some("load".to_owned()),
        repeat: Some(repeat),
        cause,
        ..failure_of(phase)
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
