//! The measures: medians, spreads, verdicts, the setup origin and the pool.

use std::time::Duration;

use report::{
    BeforeOutcome, Cause, Failure, Flag, Phase, Previous, Raw, Report, Role, SetupFrom, Src,
    Verdict,
};
use sandbox::Stopped;

use super::{
    Folder, PREVIOUS, RUN, context, instance, measure, metrics, names, port, raw, repeats, report,
    report_of, setup_side, verdicts,
};

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

    // The pool's "before", noisy and short, is no fact of a run whose "before" failed.
    let noisy = Some(repeats(&folder, "pooled-before", &[100, 300]));
    let pool = [self::report(&self::raw(
        &folder,
        repeats(&folder, "pooled-head", &[100, 100, 100]),
        noisy,
    ))];
    let pooled = report_of(&raw, &context(&raw, Previous::None, &pool), &[]);
    let mut flags = pooled.flags.iter();
    assert!(
        !flags.any(|flag| matches!(flag, Flag::HighNoise { .. } | Flag::LowRepeats { .. })),
        "{:?}",
        pooled.flags
    );
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
