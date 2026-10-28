use budget::{BudgetError, Demand, Host, Refusal, Resource, Share, check, disk_low};
use camino::Utf8Path;

const GIB: u64 = 1 << 30;

/// 8 CPUs, 16 GiB with 12 free, 100 GiB of free disk.
fn host() -> Host {
    Host {
        cpus: 8,
        memory: 16 * GIB,
        free_memory: 12 * GIB,
        free_disk: 100 * GIB,
    }
}

fn half() -> Share {
    Share::new(None).unwrap()
}

fn refused(demand: Demand, host: &Host) -> Refusal {
    check(demand, host, half()).expect_err("refused")
}

#[test]
fn check_admits_a_demand_within_the_share() {
    let admission = check(
        Demand {
            cpus: 4,
            memory: 8 * GIB,
        },
        &host(),
        half(),
    )
    .unwrap();
    assert_eq!((admission.cpus, admission.memory), (4, 8 * GIB));
}

#[test]
fn check_refuses_with_the_numbers_of_the_resource() {
    let cpu = refused(
        Demand {
            cpus: 5,
            memory: GIB,
        },
        &host(),
    );
    assert_eq!(
        (
            cpu.resource,
            cpu.requested,
            cpu.allowed,
            cpu.total,
            cpu.share
        ),
        (Resource::Cpu, 5, 4, 8, 50)
    );
    assert_eq!(
        cpu.to_string(),
        "cpu: requested 5 vCPUs, allowed 4: 50% of 8 logical CPUs"
    );

    let over_share = refused(
        Demand {
            cpus: 1,
            memory: 8 * GIB + 1,
        },
        &host(),
    );
    assert_eq!(
        (
            over_share.resource,
            over_share.requested,
            over_share.allowed,
            over_share.total,
            over_share.free
        ),
        (Resource::Memory, 8 * GIB + 1, 8 * GIB, 16 * GIB, 12 * GIB)
    );

    let busy = Host {
        free_memory: 5 * GIB,
        ..host()
    };
    let over_free = refused(
        Demand {
            cpus: 1,
            memory: 3 * GIB + 1,
        },
        &busy,
    );
    assert_eq!(
        (over_free.resource, over_free.allowed, over_free.free),
        (Resource::Memory, 3 * GIB, 5 * GIB)
    );
    assert_eq!(
        over_free.to_string(),
        "memory: requested 3.0 GiB, allowed 3.0 GiB: the lower of 50% of 16.0 GiB and 5.0 GiB \
         free less 2.0 GiB for the host"
    );

    let full = Host {
        free_disk: 19 * GIB,
        ..host()
    };
    let disk = refused(
        Demand {
            cpus: 1,
            memory: GIB,
        },
        &full,
    );
    assert_eq!(
        (disk.resource, disk.requested, disk.free),
        (Resource::Disk, 20 * GIB, 19 * GIB)
    );
    assert_eq!(
        disk.to_string(),
        "disk: 19.0 GiB free on the volume of the stand's folder, under the 20.0 GiB it keeps \
         free for snapshots"
    );
}

#[test]
fn on_a_small_host_the_share_rounds_down() {
    let small = Host {
        cpus: 3,
        memory: 3 * GIB,
        free_memory: 5 * GIB / 2,
        free_disk: 100 * GIB,
    };
    let refusal = refused(
        Demand {
            cpus: 2,
            memory: 512 << 20,
        },
        &small,
    );
    assert_eq!((refusal.resource, refusal.allowed), (Resource::Cpu, 1));
    let refusal = refused(
        Demand {
            cpus: 1,
            memory: 1536 << 20,
        },
        &small,
    );
    assert_eq!(
        refusal.to_string(),
        "memory: requested 1.5 GiB, allowed 512 MiB: the lower of 50% of 3.0 GiB and 2.5 GiB \
         free less 2.0 GiB for the host"
    );
}

#[test]
fn disk_low_is_under_20_gib_free() {
    let free = |gib: u64| Host {
        free_disk: gib * GIB,
        ..host()
    };
    assert!(disk_low(&free(19)));
    assert!(!disk_low(&free(21)));
}

#[test]
fn a_share_is_50_by_default_and_from_1_to_80() {
    assert_eq!(Share::new(None).unwrap().percent(), 50);
    assert_eq!(Share::new(Some(80)).unwrap().percent(), 80);
    assert_eq!(Share::new(Some(1)).unwrap().percent(), 1);
    for outside in [0, 81] {
        let Err(BudgetError::Share(percent)) = Share::new(Some(outside)) else {
            panic!("{outside} passes");
        };
        assert_eq!(percent, outside);
    }
}

#[test]
fn host_read_gives_numbers_of_this_host() {
    let dir = tempfile::tempdir().unwrap();
    let host = Host::read(Utf8Path::from_path(dir.path()).unwrap()).unwrap();
    assert!(host.cpus > 0, "{host:?}");
    assert!(host.memory > 0, "{host:?}");
    assert!(host.free_memory > 0, "{host:?}");
    assert!(host.free_disk > 0, "{host:?}");
    assert!(host.free_memory <= host.memory, "{host:?}");
}
