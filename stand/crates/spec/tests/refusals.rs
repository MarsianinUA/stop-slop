use std::fs;

use camino::Utf8Path;
use spec::{Mount, Policy, load};

mod common;

use common::{assert_refused, fixture};

#[test]
fn a_volume_name_keeps_to_the_rule_of_docker() {
    assert_refused(&[
        (
            "volume-name",
            "volumes.a/../../x",
            6,
            "`a/../../x` is not a volume name",
        ),
        (
            "volume-name-short",
            "services.load.volumes[0]",
            4,
            "`a/../../x` is not a volume name",
        ),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let file = Utf8Path::from_path(dir.path())
        .unwrap()
        .join("compose.yaml");
    let example = fs::read_to_string(fixture("example")).unwrap();
    fs::write(&file, example.replace("pgdata", "pg.data-1")).unwrap();
    let stand = load(&file, &Policy::new(None).unwrap()).unwrap();
    let (_, db) = stand
        .services
        .iter()
        .find(|(name, _)| name.as_str() == "db")
        .unwrap();
    assert!(
        matches!(&db.mounts[..], [Mount::Volume { name, .. }] if name == "pg.data-1"),
        "{:?}",
        db.mounts
    );
}

#[test]
fn a_name_is_one_file_name_on_every_host() {
    assert_refused(&[
        (
            "name-case",
            "services.Api",
            5,
            "`Api` differs from `api` only in case",
        ),
        (
            "probe-case",
            "x-stand.probes.Rows",
            15,
            "`Rows` differs from `rows` only in case",
        ),
        (
            "volume-case",
            "volumes.PGData",
            12,
            "`PGData` differs from `pgdata` only in case",
        ),
        (
            "name-device",
            "services.aux",
            2,
            "`aux` is a device name on Windows",
        ),
        (
            "volume-device",
            "volumes.nul.data",
            10,
            "`nul.data` is a device name on Windows",
        ),
        (
            "volume-trailing-dot",
            "volumes.data.",
            10,
            "`data.` ends with a `.`, which Windows drops",
        ),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let file = Utf8Path::from_path(dir.path())
        .unwrap()
        .join("compose.yaml");
    let example = fs::read_to_string(fixture("example")).unwrap();
    fs::write(&file, example.replace("pgdata", "nul-data")).unwrap();
    load(&file, &Policy::new(None).unwrap()).unwrap();
}

#[test]
fn a_setup_service_and_the_workload_have_one_replica() {
    assert_refused(&[
        (
            "setup-replicas",
            "services.seed.deploy.replicas",
            6,
            "`seed` is a setup service",
        ),
        (
            "prepare-replicas",
            "services.prepare.deploy.replicas",
            7,
            "`prepare` is a setup service",
        ),
        (
            "workload-replicas",
            "services.load.deploy.replicas",
            6,
            "`load` is the workload",
        ),
    ]);
}

#[test]
fn an_unchanged_file_is_a_path_in_stand_out_or_the_output_of_an_after_workload_probe() {
    let file = "x-stand.expect.unchanged[0].file";
    assert_refused(&[
        (
            "unchanged-absolute",
            file,
            10,
            "`/stand/out/orders.jsonl` is absolute",
        ),
        ("unchanged-parent", file, 10, "has `..`"),
        ("unchanged-empty-part", file, 10, "has an empty part"),
        ("unchanged-backslash", file, 10, "has a `\\`"),
        ("unchanged-colon", file, 10, "has a `:`"),
        ("unchanged-not-jsonl", file, 10, "not a `*.jsonl` file"),
        (
            "unchanged-unknown-probe",
            file,
            10,
            "`x-stand.probes` has none",
        ),
        ("unchanged-probe-on-failure", file, 16, "runs on-failure"),
    ]);
}

#[test]
fn a_harness_source_is_in_the_folder_of_the_stand_file() {
    assert_refused(&[(
        "harness-missing",
        "services.load.volumes[0]",
        4,
        "`./scripts` is not in the folder of the stand file",
    )]);
}

#[test]
fn a_fault_targets_a_long_running_service() {
    assert_refused(&[
        (
            "fault-setup",
            "x-stand.fault.target",
            13,
            "`seed` is a setup service",
        ),
        (
            "fault-workload",
            "x-stand.fault.target",
            8,
            "`load` is the workload",
        ),
    ]);
}
