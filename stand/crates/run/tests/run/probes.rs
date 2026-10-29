//! The probes at their taps and the cut before them.

use std::collections::BTreeSet;
use std::fs;

use camino::Utf8Path;
use report::BeforeOutcome;
use run::Side;
use sandbox::{Call, ExecOutcome, Mount, Script};

use super::{
    Case, EXAMPLE, Hooked, SECOND, app_failure, assert_clean, created, example_rules, execs, exits,
    fake, metrics_in, ran, replaced, stops, vm, with,
};

/// `EXAMPLE` with an `on-failure` probe in `api`, which no setup starts.
fn traced() -> String {
    EXAMPLE.replace(
        "  probes:\n",
        "  probes:\n    trace: { in: api, at: on-failure, run: [\"cat\", \"/tmp/trace\"] }\n",
    )
}

/// No `Stop` from the last `Create` before `index` to `index`: a probe there comes before the
/// stops of its phase.
fn before_the_stops(journal: &[Call], index: usize) -> bool {
    let phase = journal[..index]
        .iter()
        .rposition(|call| matches!(call, Call::Create { .. }));
    let phase = &journal[phase.unwrap()..index];
    !phase.iter().any(|call| matches!(call, Call::Stop(_)))
}

#[test]
fn probes_run_at_their_taps_and_write_into_out() {
    let case = Case::new(EXAMPLE);
    let fake = fake(example_rules());
    let sandbox = Hooked::new(&fake);
    let raw = sandbox.run(&case, &case.versions(true)).unwrap();
    let setups = [
        (Side::Head, &raw.setup.head),
        (Side::Before, raw.setup.before.as_ref().unwrap()),
    ];
    for (side, setup) in setups {
        assert_eq!(ran(&setup.probes), ["schema"], "{side}");
        let schema = &setup.probes[0];
        let out = case.run_dir().join(format!("setup-{side}/out"));
        assert_eq!(schema.out, out.join("probe.schema.jsonl"));
        assert_eq!(schema.outcome, ExecOutcome::Exited(0));
        assert_eq!(fs::read(&schema.out).unwrap(), b"{\"table\":\"orders\"}\n");
        assert!(
            out.join("probe.schema.jsonl.err").is_file(),
            "its stderr beside it"
        );
    }
    let Some(BeforeOutcome::Passed(before)) = &raw.before else {
        panic!("{:?}", raw.before)
    };
    for output in raw.head.iter().chain(before) {
        assert_eq!(ran(&output.probes), ["orders"], "repeat {}", output.repeat);
        let orders = &output.probes[0];
        assert_eq!(orders.out, output.out_dir.join("probe.orders.jsonl"));
        assert_eq!(fs::read(&orders.out).unwrap(), b"{\"id\":1}\n");
        assert!(
            output.out_dir.join("orders.jsonl").is_file(),
            "the workload's file"
        );
    }
    let files: Vec<&str> = raw.probes.keys().map(String::as_str).collect();
    assert_eq!(
        files,
        [
            "probe.last.jsonl",
            "probe.orders.jsonl",
            "probe.schema.jsonl"
        ]
    );
    let orders = &raw.probes["probe.orders.jsonl"];
    assert_eq!(
        (orders.key.as_deref(), orders.ignore.as_slice()),
        (Some("/id"), ["/at".to_owned()].as_slice())
    );
    // Each in replica 1 of `db` with its own `run` and 30 s to go, before the stops of its phase:
    // `schema` once per setup, `orders` in each repeat, `last` never.
    let journal = fake.journal();
    let mut phases = Vec::new();
    for (index, stdout, vm) in execs(&journal) {
        assert_eq!(vm, "db-1");
        assert!(before_the_stops(&journal, index), "{stdout}");
        let Call::Exec { argv, .. } = &journal[index] else {
            unreachable!()
        };
        let phase = stdout.parent().unwrap().parent().unwrap().file_name();
        phases.push((phase.unwrap().to_owned(), argv[3].as_str()));
    }
    let mut expected = vec![
        ("setup-before".to_owned(), "select 1"),
        ("setup-head".to_owned(), "select 1"),
    ];
    for repeat in 1..=3 {
        for side in ["before", "head"] {
            expected.push((format!("repeat-{repeat}-{side}"), "select 2"));
        }
    }
    assert_eq!(phases, expected);
    assert_eq!(*sandbox.deadlines.borrow(), [30 * SECOND; 2 + 2 * 3]);
    assert_clean(&fake, &case.left());
}

#[test]
fn the_cut_reads_every_instance_before_the_first_probe() {
    let case = Case::new(EXAMPLE);
    let fake = fake(example_rules());
    let sandbox = Hooked {
        logs_grow: true,
        ..Hooked::new(&fake)
    };
    let raw = sandbox.run(&case, &case.versions(false)).unwrap();
    let journal = fake.journal();
    let made = created(&journal);
    let probes: Vec<usize> = execs(&journal).iter().map(|(index, ..)| *index).collect();
    let mut cuts = Vec::new();
    for probe in probes {
        let before = &journal[..probe];
        // The measured window ends with the last `wait`: the exit of the workload, or of the last
        // setup service.
        let window = before
            .iter()
            .rposition(|call| matches!(call, Call::Wait { .. }));
        let cut = &before[window.unwrap()..];
        let mut read = Vec::new();
        for (launch, id) in &made {
            let live = before
                .iter()
                .any(|call| matches!(call, Call::Create { ids, .. } if ids.contains(id)))
                && !before
                    .iter()
                    .any(|call| matches!(call, Call::Destroy(gone) if gone == id));
            if !live {
                continue;
            }
            let metrics = cut
                .iter()
                .any(|call| matches!(call, Call::Metrics(of) if of == id));
            let logs = cut
                .iter()
                .any(|call| matches!(call, Call::Logs(of) if of == id));
            assert!(metrics && logs, "{}", vm(launch));
            read.push(vm(launch));
        }
        cuts.push(read);
    }
    let repeat = ["db-1", "api-1", "api-2", "load-1"];
    assert_eq!(
        cuts,
        [
            &["db-1", "prepare-1", "migrate-1"][..],
            &repeat,
            &repeat,
            &repeat
        ]
    );
    // Each read of a log is a line longer than the one before: the cut read one line, the drain
    // wrote it with what came after.
    let first_line = |log: &Utf8Path| {
        let bytes = fs::read(log).unwrap();
        let line = bytes.iter().position(|&byte| byte == b'\n').unwrap() + 1;
        assert!(bytes.len() > line, "{log} has what came after the cut");
        line as u64
    };
    for output in &raw.head {
        let dir = output.out_dir.parent().unwrap();
        for (instance, written) in output.instances.iter().zip(metrics_in(dir)) {
            assert_eq!(instance.log_cut, first_line(&instance.log), "{dir}");
            assert_eq!(written["logCut"], instance.log_cut);
        }
    }
    let setup = case.run_dir().join("setup-head");
    for written in metrics_in(&setup) {
        let vm = format!(
            "{}-{}",
            written["service"].as_str().unwrap(),
            written["replica"]
        );
        let log = setup.join(format!("{vm}.log"));
        assert_eq!(written["logCut"], first_line(&log), "{vm}");
    }
    assert_clean(&fake, &case.left());
}

#[test]
fn on_failure_probes_run_before_the_stop_and_their_outputs_are_in_the_failure() {
    // The workload fails in the first repeat: `last` in `db` and `trace` in replica 1 of `api`
    // run there instead of `orders`.
    let case = Case::new(&traced());
    let failing = fake(with("load", exits(4, SECOND)));
    let failed = app_failure(case.run(&failing, &case.versions(false)));
    let out = case.run_dir().join("repeat-1-head/out");
    let outputs = [out.join("probe.last.jsonl"), out.join("probe.trace.jsonl")];
    assert_eq!(failed.failure.probes, outputs);
    assert_eq!(ran(&failed.raw.head[0].probes), ["last", "trace"]);
    let journal = failing.journal();
    let probed = execs(&journal);
    let vms: Vec<(&str, &str)> = probed
        .iter()
        .map(|(_, stdout, vm)| (stdout.file_name().unwrap(), vm.as_str()))
        .collect();
    assert_eq!(
        vms,
        [
            ("probe.schema.jsonl", "db-1"),
            ("probe.last.jsonl", "db-1"),
            ("probe.trace.jsonl", "api-1")
        ]
    );
    for (index, stdout, _) in &probed {
        assert!(before_the_stops(&journal, *index), "{stdout}");
    }
    assert_clean(&failing, &case.left());

    // `migrate` fails in setup: `last` runs, `trace` does not, since no setup starts `api`.
    let case = Case::new(&traced());
    let failing = fake(with("migrate", exits(1, 2 * SECOND)));
    let failed = app_failure(case.run(&failing, &case.versions(false)));
    let out = case.run_dir().join("setup-head/out/probe.last.jsonl");
    assert_eq!(failed.failure.probes, [out]);
    assert_eq!(ran(&failed.raw.setup.head.probes), ["last"]);
    let journal = failing.journal();
    let probed = execs(&journal);
    assert_eq!(probed.len(), 1);
    let (index, _, vm) = &probed[0];
    assert_eq!(vm, "db-1");
    assert!(before_the_stops(&journal, *index));
    assert_clean(&failing, &case.left());
}

#[test]
fn a_probe_that_times_out_or_exits_nonzero_is_what_it_saw() {
    let case = Case::new(EXAMPLE);
    let db = Script {
        volume_bytes: 4096,
        probes: vec![
            (
                "probe.schema.jsonl".into(),
                ExecOutcome::Exited(2),
                b"half\n".to_vec(),
            ),
            (
                "probe.orders.jsonl".into(),
                ExecOutcome::TimedOut,
                Vec::new(),
            ),
            ("probe.last.jsonl".into(), ExecOutcome::TimedOut, Vec::new()),
        ],
        ..stops(SECOND)
    };
    let saw = fake(with("db", db.clone()));
    let raw = case.run(&saw, &case.versions(false)).unwrap();
    let schema = &raw.setup.head.probes[0];
    assert_eq!(schema.outcome, ExecOutcome::Exited(2));
    assert_eq!(fs::read(&schema.out).unwrap(), b"half\n");
    assert_eq!(raw.head.len(), 3, "the run goes on");
    for output in &raw.head {
        assert_eq!(output.probes[0].outcome, ExecOutcome::TimedOut);
        assert!(output.probes[0].out.is_file());
    }
    assert_clean(&saw, &case.left());

    // An `on-failure` probe past its deadline is still an output of the failure.
    let case = Case::new(EXAMPLE);
    let saw = fake(replaced(with("db", db), "load", exits(4, SECOND)));
    let failed = app_failure(case.run(&saw, &case.versions(false)));
    let last = case.run_dir().join("repeat-1-head/out/probe.last.jsonl");
    assert_eq!(failed.failure.probes, [last]);
    assert_eq!(failed.raw.head[0].probes[0].outcome, ExecOutcome::TimedOut);
    assert_clean(&saw, &case.left());
}

#[test]
fn stand_out_goes_to_the_workload_and_every_service_with_a_probe() {
    let cases = [
        (EXAMPLE.to_owned(), &["db", "load"][..]),
        (traced(), &["api", "db", "load"][..]),
    ];
    for (recipe, probed) in cases {
        let case = Case::new(&recipe);
        let fake = fake(example_rules());
        case.run(&fake, &case.versions(true)).unwrap();
        let mut bound = BTreeSet::new();
        for (launch, _) in created(&fake.journal()) {
            let outs: Vec<(&Utf8Path, bool)> = launch
                .mounts
                .iter()
                .filter_map(|mount| match mount {
                    Mount::Bind {
                        host,
                        guest,
                        read_only,
                    } if guest == "/stand/out" => Some((host.as_path(), *read_only)),
                    _ => None,
                })
                .collect();
            match outs.as_slice() {
                [] => {}
                // The `out/` of its phase, writable.
                [(host, false)] => {
                    assert_eq!(host.file_name(), Some("out"));
                    let phase = host.parent().unwrap();
                    assert_eq!(phase.parent(), Some(case.run_dir().as_path()));
                    bound.insert(launch.service.as_str().to_owned());
                }
                other => panic!("{}: {other:?}", launch.service),
            }
        }
        let probed: BTreeSet<String> = probed.iter().map(|name| name.to_string()).collect();
        assert_eq!(bound, probed);
        assert_clean(&fake, &case.left());
    }
}
