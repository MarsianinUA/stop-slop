//! The behaviour of the results and the probes, `read_outputs`, `write` and `load`.

use std::fs;

use camino::Utf8PathBuf;
use report::{
    At, BadResult, BeforeOutcome, Cause, DeadReason, Failure, Flag, PassState, Phase, Previous,
    RESULT_LIMIT, Raw, RepeatOutput, Report, ReportError, SecondPassOutcome, Src, load,
    read_outputs,
};
use sandbox::ExecOutcome;
use serde_json::{Value, json};

use super::{
    Folder, PREVIOUS, RUN, behaviour, context, pass, pass_file, passed, probe_ran, probe_rule, raw,
    repeats, report, report_of, setup_side, with_files, written,
};

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

/// The second pass masks what its version's measured repeats change, the pool's among them, with
/// no "before" too (batch review 6).
#[test]
fn the_pool_teaches_the_second_pass_mask_without_before() {
    let folder = Folder::new();
    let totals = |name: &str, at: &[u32]| {
        let texts: Vec<String> = at.iter().map(|at| format!("{{\"at\":{at}}}\n")).collect();
        let files: Vec<[(&str, &str); 1]> = texts
            .iter()
            .map(|text| [("totals.jsonl", text.as_str())])
            .collect();
        let lists: Vec<&[(&str, &str)]> = files.iter().map(|file| &file[..]).collect();
        with_files(&folder, name, &lists)
    };
    let earlier = raw(&folder, totals("a", &[1, 2]), None);
    let mut earlier_context = context(&earlier, Previous::None, &[]);
    earlier_context.run = PREVIOUS.to_owned();
    let (pooled, dir) = written(&folder, "run-a", &earlier, &earlier_context);
    let mut raw = raw(&folder, totals("head", &[3]), None);
    let pass_of = |n: u32, at: u32| {
        let text = format!("{{\"at\":{at}}}\n");
        pass(&folder, "head", n, &[("totals.jsonl", text.as_str())], &[])
    };
    raw.second_pass = Some(SecondPassOutcome {
        unchanged: vec!["totals.jsonl".to_owned()],
        head: passed(pass_of(1, 5), pass_of(2, 6)),
        before: None,
    });

    let alone = report(&raw);
    assert_eq!(pass_file(&alone, "totals.jsonl").head, PassState::Changed);
    let pool = [pooled];
    let together = report_of(&raw, &context(&raw, Previous::None, &pool), &[dir]);
    assert_eq!(pass_file(&together, "totals.jsonl").head, PassState::Same);
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
        matches!(&error, ReportError::BadResult(BadResult::NotPlain(path)) if *path == odd),
        "{error}"
    );
}

/// A result over `RESULT_LIMIT` is refused by its size before it is read; one of the limit reads
/// (batch review 6).
#[test]
fn a_result_over_the_limit_is_refused_before_it_is_read() {
    let folder = Folder::new();
    let head = with_files(&folder, "head", &[&[("out.jsonl", "{}\n")]]);
    let big = head[0].out_dir.join("big.jsonl");
    let file = fs::File::create(&big).expect("a result file");
    file.set_len(RESULT_LIMIT).expect("a result of the limit");
    let raw = raw(&folder, head, None);
    assert!(read_outputs(&raw, &[]).is_ok());

    file.set_len(RESULT_LIMIT + 1)
        .expect("a result over the limit");
    let error = read_outputs(&raw, &[]).expect_err("a result over the limit");
    assert!(
        matches!(
            &error,
            ReportError::BadResult(BadResult::TooLarge { path, size })
                if *path == big && *size == RESULT_LIMIT + 1
        ),
        "{error}"
    );
    assert!(error.to_string().contains("64 MiB"), "{error}");
}

/// No JSON could name it; the setup cache skips it too (batch review 6).
#[test]
fn a_name_that_is_not_utf8_in_out_is_skipped() {
    let folder = Folder::new();
    let head = with_files(&folder, "head", &[&[("out.jsonl", "{}\n")]]);
    #[cfg(unix)]
    let odd = {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::OsStr::from_bytes(b"odd-\xff.jsonl").to_owned()
    };
    #[cfg(windows)]
    let odd = {
        use std::os::windows::ffi::OsStringExt;
        let mut wide: Vec<u16> = "odd-".encode_utf16().collect();
        wide.push(0xD800);
        wide.extend(".jsonl".encode_utf16());
        std::ffi::OsString::from_wide(&wide)
    };
    // APFS refuses such a name: macOS has nothing to skip.
    if fs::write(head[0].out_dir.as_std_path().join(odd), "broken\n").is_err() {
        return;
    }
    let before = with_files(&folder, "before", &[&[("out.jsonl", "{}\n")]]);
    let raw = raw(&folder, head, Some(before));
    let report = report(&raw);
    let behaviour = report.behaviour.iter().flatten();
    let files: Vec<&str> = behaviour.map(|file| file.file.as_str()).collect();
    assert_eq!(files, ["out.jsonl"]);
    let mut flags = report.flags.iter();
    assert!(!flags.any(|flag| matches!(flag, Flag::BadResultLine { .. })));
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
        matches!(&error, ReportError::BadResult(BadResult::NotPlain(path)) if *path == link),
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
