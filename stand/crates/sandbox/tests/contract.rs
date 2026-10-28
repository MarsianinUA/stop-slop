//! The contract set. Each shared case is a function over `&impl Sandbox` that makes its own folders
//! and leaves nothing behind, so it runs as is on every backend: `fake_<case>` runs it on a `Fake`
//! whose scenario mirrors the case's commands by service name. The cases use real small images and
//! real commands, and check only what holds in real time too: who exited, with what code, in what
//! order, `TimedOut`, `Gone`; never a virtual instant. The last three cases check the `Fake` itself.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::fs;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use sandbox::{
    Call, ExecOutcome, Fake, Fault, FaultMode, ImageRef, Launch, Metrics, Mount, Outbound, Rule,
    Sandbox, SandboxError, SandboxId, Scenario, Script, Seed, Stopped, VolumeName, Waitable,
};
use tempfile::TempDir;

const ALPINE: &str = "alpine:3.20";
const NODE: &str = "node:22-alpine";
/// Its `USER` is 101, not root.
const NGINX: &str = "nginxinc/nginx-unprivileged:alpine";
/// A Docker Hub account name has no `-`, so no one can push this.
const MISSING: &str = "stop-slop-stand/no-such-image:1";
/// Enough for a short command in a fresh VM to run and exit.
const EXIT: Duration = Duration::from_secs(60);

fn create_batch_and_wait(sandbox: &impl Sandbox) {
    let mut quick = launch("quick", ALPINE);
    quick.command = sh("exit 3");
    let mut slow = launch("slow", ALPINE);
    slow.command = Some(argv(&["sleep", "600"]));
    let started = sandbox.now();
    let ids = sandbox.create(&[quick, slow]).unwrap();
    let [quick, slow] = ids.as_slice() else {
        panic!("two ids for a batch of two: {ids:?}");
    };
    let both = [
        Waitable::Sandbox(quick.clone()),
        Waitable::Sandbox(slow.clone()),
    ];
    let exited = sandbox
        .wait(&both, sandbox.now() + EXIT)
        .unwrap()
        .expect("quick exits");
    assert_eq!((&exited.what, exited.code), (&both[0], Some(3)));
    assert!(
        started <= exited.at && exited.at <= sandbox.now(),
        "{exited:?}"
    );

    let check = sandbox
        .exec_detached(slow, &argv(&["sh", "-c", "exit 4"]))
        .unwrap();
    let check = Waitable::Exec(check);
    let ended = sandbox
        .wait(std::slice::from_ref(&check), sandbox.now() + EXIT)
        .unwrap();
    let ended = ended.expect("the healthcheck ends");
    assert_eq!((ended.what, ended.code), (check, Some(4)));

    let deadline = sandbox.now() + Duration::from_secs(1);
    assert!(sandbox.wait(&both[1..], deadline).unwrap().is_none());
    assert!(sandbox.now() >= deadline);
    destroy(sandbox, &ids);
}

#[test]
fn fake_create_batch_and_wait() {
    on_fake(
        vec![
            rule("quick", exits(3)),
            rule(
                "slow",
                Script {
                    health: vec![4],
                    ..Script::default()
                },
            ),
        ],
        create_batch_and_wait,
    );
}

fn logs_and_metrics_until_destroy(sandbox: &impl Sandbox) {
    let mut chatty = launch("chatty", ALPINE);
    chatty.command = sh("echo hello; sleep 600");
    let id = create(sandbox, chatty);
    sandbox.logs(&id).unwrap();
    sandbox.metrics(&id).unwrap();
    sandbox.destroy(&id).unwrap();
    assert_gone(sandbox.logs(&id), &id);
    assert_gone(sandbox.metrics(&id), &id);
    sandbox.destroy(&id).unwrap();
}

#[test]
fn fake_logs_and_metrics_until_destroy() {
    on_fake(
        vec![rule("chatty", Script::default())],
        logs_and_metrics_until_destroy,
    );
}

fn stop_then_the_rest_work(sandbox: &impl Sandbox) {
    let data = VolumeName::new("data");
    let mut worker = launch("worker", ALPINE);
    worker.command = sh("trap 'exit 0' TERM; while true; do sleep 1; done");
    worker.mounts = vec![volume(&data, "/data")];
    let grace = worker.stop_grace;
    let id = create(sandbox, worker);
    let stopped = sandbox.stop(&id).unwrap();
    assert!(!stopped.killed && stopped.after < grace, "{stopped:?}");
    let sizes = sandbox.volume_sizes(&id).unwrap();
    assert_eq!(names(&sizes), std::slice::from_ref(&data));
    let snapshot = sandbox.snapshot(&id).unwrap();
    sandbox.logs(&id).unwrap();
    sandbox.metrics(&id).unwrap();
    sandbox.destroy(&id).unwrap();
    sandbox.drop_snapshot(&snapshot).unwrap();
    sandbox.drop_volumes(&[data]).unwrap();
}

#[test]
fn fake_stop_then_the_rest_work() {
    let script = Script {
        stop: Stopped {
            after: Duration::from_millis(300),
            killed: false,
        },
        volume_bytes: 4096,
        ..Script::default()
    };
    on_fake(vec![rule("worker", script)], stop_then_the_rest_work);
}

fn snapshot_seeds_a_volume(sandbox: &impl Sandbox) {
    let (_first, first) = folder();
    let (_second, second) = folder();
    let data = VolumeName::new("data");
    let cache = VolumeName::new("cache");
    let volumes = [volume(&data, "/data"), volume(&cache, "/cache")];
    let mut seeder = launch("seeder", ALPINE);
    seeder.command = sh(
        "echo setup > /data/file && head -c 1048576 /dev/zero > /data/blob && echo cached > /cache/file",
    );
    seeder.mounts = [&volumes[..], &[out(&first)]].concat();
    let seeder = create(sandbox, seeder);
    assert_eq!(wait_exit(sandbox, &seeder), Some(0));
    sandbox.stop(&seeder).unwrap();
    let sizes = sandbox.volume_sizes(&seeder).unwrap();
    assert_eq!(names(&sizes), [cache.clone(), data.clone()]);
    let data_bytes = sizes.iter().find(|(name, _)| *name == data).unwrap().1;
    assert!(data_bytes >= 1 << 20, "{sizes:?}");
    let snapshot = sandbox.snapshot(&seeder).unwrap();
    sandbox.destroy(&seeder).unwrap();

    let mut reader = launch("reader", ALPINE);
    reader.seed = Some(Seed::Snapshot(snapshot.clone()));
    reader.mounts = [&volumes[..], &[out(&second)]].concat();
    reader.command = sh("cat /data/file /cache/file > /stand/out/seen");
    let reader = create(sandbox, reader);
    assert_eq!(wait_exit(sandbox, &reader), Some(0));
    assert_eq!(read(&second.join("seen")), "setup\ncached\n");
    sandbox.destroy(&reader).unwrap();
    sandbox.drop_snapshot(&snapshot).unwrap();
    sandbox.drop_volumes(&[data, cache]).unwrap();
}

#[test]
fn fake_snapshot_seeds_a_volume() {
    let seeder = Script {
        volume_bytes: 1 << 21,
        ..exits(0)
    };
    let reader = writes(exits(0), &[("/stand/out/seen", "setup\ncached\n")]);
    on_fake(
        vec![rule("seeder", seeder), rule("reader", reader)],
        snapshot_seeds_a_volume,
    );
}

/// `$WHO`, the vCPUs and the memory in MiB, as the guest sees them.
const LIMITS: &str = r#"echo "$WHO $(nproc) $(awk '/^MemTotal/ {print int($2 / 1024)}' /proc/meminfo)" > /stand/out/seen"#;

fn restore_takes_its_own_launch(sandbox: &impl Sandbox) {
    let (_first, first) = folder();
    let (_second, second) = folder();
    let mut prepare = launch("prepare", ALPINE);
    prepare.memory = 1 << 30;
    prepare.env.insert("WHO".into(), "prepare".into());
    prepare.mounts = vec![out(&first)];
    prepare.command = sh(LIMITS);
    let prepare = create(sandbox, prepare);
    assert_eq!(wait_exit(sandbox, &prepare), Some(0));
    let snapshot = sandbox.snapshot(&prepare).unwrap();
    sandbox.destroy(&prepare).unwrap();

    let mut api = launch("api", ALPINE);
    api.cpus = 2;
    api.memory = 512 << 20;
    api.env.insert("WHO".into(), "api".into());
    api.mounts = vec![out(&second)];
    api.command = sh(LIMITS);
    api.seed = Some(Seed::Snapshot(snapshot.clone()));
    let api = create(sandbox, api);
    assert_eq!(wait_exit(sandbox, &api), Some(0));
    let (who, cpus, mib) = limits(&read(&first.join("seen")));
    assert!(
        who == "prepare" && cpus == 1 && mib > 600,
        "the prepare VM: {who} {cpus} {mib}"
    );
    let (who, cpus, mib) = limits(&read(&second.join("seen")));
    assert!(
        who == "api" && cpus == 2 && mib < 600,
        "the restored VM: {who} {cpus} {mib}"
    );

    let stdout = second.join("who.txt");
    let echo = argv(&["sh", "-c", "echo $WHO"]);
    let outcome = sandbox
        .exec(&api, &echo, sandbox.now() + EXIT, &stdout)
        .unwrap();
    assert_eq!(
        (outcome, read(&stdout)),
        (ExecOutcome::Exited(0), "api\n".to_owned())
    );
    sandbox.destroy(&api).unwrap();
    sandbox.drop_snapshot(&snapshot).unwrap();
}

#[test]
fn fake_restore_takes_its_own_launch() {
    let prepare = writes(exits(0), &[("/stand/out/seen", "prepare 1 985\n")]);
    let mut api = writes(exits(0), &[("/stand/out/seen", "api 2 480\n")]);
    api.probes = vec![("who.txt".into(), ExecOutcome::Exited(0), b"api\n".to_vec())];
    on_fake(
        vec![rule("prepare", prepare), rule("api", api)],
        restore_takes_its_own_launch,
    );
}

fn restore_refuses_a_missing_bind_and_a_new_volume(sandbox: &impl Sandbox) {
    let (_first, first) = folder();
    let (_second, second) = folder();
    let data = VolumeName::new("data");
    let extra = VolumeName::new("extra");
    let mut keeper = launch("keeper", ALPINE);
    keeper.command = sh("echo kept > /data/file");
    keeper.mounts = vec![volume(&data, "/data"), out(&first)];
    let id = create(sandbox, keeper.clone());
    assert_eq!(wait_exit(sandbox, &id), Some(0));
    sandbox.stop(&id).unwrap();
    let snapshot = sandbox.snapshot(&id).unwrap();
    sandbox.destroy(&id).unwrap();

    let mut restored = keeper;
    restored.seed = Some(Seed::Snapshot(snapshot.clone()));
    restored.mounts = vec![volume(&data, "/data")];
    let refused = sandbox.create(std::slice::from_ref(&restored));
    assert!(
        matches!(refused, Err(SandboxError::Backend { .. })),
        "without the bind of /stand/out: {refused:?}"
    );
    restored.mounts = vec![
        volume(&data, "/data"),
        volume(&extra, "/extra"),
        out(&second),
    ];
    let refused = sandbox.create(&[restored]);
    assert!(
        matches!(refused, Err(SandboxError::Backend { .. })),
        "with a new volume: {refused:?}"
    );
    let left = sandbox.leftovers().unwrap();
    assert!(left.sandboxes.is_empty(), "a refused restore left {left:?}");
    sandbox.drop_snapshot(&snapshot).unwrap();
    sandbox.drop_volumes(&[data, extra]).unwrap();
}

#[test]
fn fake_restore_refuses_a_missing_bind_and_a_new_volume() {
    on_fake(
        vec![rule("keeper", exits(0))],
        restore_refuses_a_missing_bind_and_a_new_volume,
    );
}

const RUN_SH: &str = "#!/bin/sh\necho run\n";
/// Above the 4 MiB that one frame of `stdin_bytes` carries.
const BIG: usize = 5 << 20;
/// What the service sees of the tree as it starts, then a file of its own on the version disk.
const LOOK: &str = r#"cd /w/app
stat -c '%n %a %s' run.sh big.bin > /stand/out/start.txt
readlink link >> /stand/out/start.txt
for f in run.sh big.bin link; do
  [ "$(stat -c %u "$f")" = "$(id -u)" ] || echo "$f is not the image user's" >> /stand/out/start.txt
done
echo built > built.txt"#;

/// What `LOOK` writes when every file of `write_tree` is in place and the image user's.
fn looked() -> String {
    format!("run.sh 755 {}\nbig.bin 644 {BIG}\nrun.sh\n", RUN_SH.len())
}

fn tree_lands_before_the_command(sandbox: &impl Sandbox) {
    let (_dir, dir) = folder();
    let (first, second) = (dir.join("first"), dir.join("second"));
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    let tar = dir.join("tree.tar");
    write_tree(&tar);
    let mut tree = launch("tree", NGINX);
    tree.seed = Some(Seed::Tree {
        tar,
        at: "/w/app".into(),
    });
    tree.mounts = vec![out(&first)];
    tree.command = sh(LOOK);
    let tree = create(sandbox, tree);
    assert_eq!(wait_exit(sandbox, &tree), Some(0));
    assert_eq!(read(&first.join("start.txt")), looked());
    // After the exit, as the prepare service's snapshot is taken.
    let snapshot = sandbox.snapshot(&tree).unwrap();
    sandbox.destroy(&tree).unwrap();

    let mut reader = launch("reader", NGINX);
    reader.seed = Some(Seed::Snapshot(snapshot.clone()));
    reader.mounts = vec![out(&second)];
    reader.command = sh(
        "cat /w/app/built.txt > /stand/out/seen && stat -c %s /w/app/big.bin >> /stand/out/seen",
    );
    let reader = create(sandbox, reader);
    assert_eq!(wait_exit(sandbox, &reader), Some(0));
    assert_eq!(read(&second.join("seen")), format!("built\n{BIG}\n"));
    sandbox.destroy(&reader).unwrap();
    sandbox.drop_snapshot(&snapshot).unwrap();
}

#[test]
fn fake_tree_lands_before_the_command() {
    let start = looked();
    let seen = format!("built\n{BIG}\n");
    on_fake(
        vec![
            rule(
                "tree",
                writes(exits(0), &[("/stand/out/start.txt", &start)]),
            ),
            rule("reader", writes(exits(0), &[("/stand/out/seen", &seen)])),
        ],
        tree_lands_before_the_command,
    );
}

fn copy_out_brings_the_scratch_folder(sandbox: &impl Sandbox) {
    let (_dir, dir) = folder();
    let mut profiled = launch("profiled", ALPINE);
    profiled.mounts = vec![Mount::Scratch {
        guest: "/stand/profile".into(),
    }];
    profiled.command = sh(
        "echo one > /stand/profile/a.txt && mkdir /stand/profile/sub && echo two > /stand/profile/sub/b.txt",
    );
    let id = create(sandbox, profiled);
    assert_eq!(wait_exit(sandbox, &id), Some(0));
    sandbox.stop(&id).unwrap();
    let host = dir.join("profile");
    fs::create_dir(&host).unwrap();
    sandbox.copy_out(&id, "/stand/profile", &host).unwrap();
    assert_eq!(read(&host.join("a.txt")), "one\n");
    assert_eq!(read(&host.join("sub").join("b.txt")), "two\n");
    sandbox.destroy(&id).unwrap();
    let again = dir.join("again");
    fs::create_dir(&again).unwrap();
    assert_gone(sandbox.copy_out(&id, "/stand/profile", &again), &id);
}

#[test]
fn fake_copy_out_brings_the_scratch_folder() {
    let files = [
        ("/stand/profile/a.txt", "one\n"),
        ("/stand/profile/sub/b.txt", "two\n"),
    ];
    on_fake(
        vec![rule("profiled", writes(exits(0), &files))],
        copy_out_brings_the_scratch_folder,
    );
}

fn create_returns_once_the_command_runs(sandbox: &impl Sandbox) {
    let (_dir, dir) = folder();
    let mut sleeper = launch("sleeper", ALPINE);
    sleeper.command = Some(argv(&["sleep", "600"]));
    sleeper.mounts = vec![out(&dir)];
    let id = create(sandbox, sleeper);
    let pgrep = argv(&["pgrep", "-x", "sleep"]);
    let seen = sandbox.exec(&id, &pgrep, sandbox.now() + EXIT, &dir.join("pgrep.txt"));
    assert_eq!(seen.unwrap(), ExecOutcome::Exited(0));
    sandbox.destroy(&id).unwrap();
}

#[test]
fn fake_create_returns_once_the_command_runs() {
    on_fake(
        vec![rule("sleeper", Script::default())],
        create_returns_once_the_command_runs,
    );
}

fn exec_writes_stdout_and_times_out(sandbox: &impl Sandbox) {
    let (_dir, dir) = folder();
    let mut target = launch("target", ALPINE);
    target.command = Some(argv(&["sleep", "600"]));
    target.mounts = vec![out(&dir)];
    let id = create(sandbox, target);
    let stdout = dir.join("said.txt");
    let say = argv(&["sh", "-c", "echo said; echo oops >&2; exit 3"]);
    let said = sandbox
        .exec(&id, &say, sandbox.now() + EXIT, &stdout)
        .unwrap();
    assert_eq!(
        (said, read(&stdout)),
        (ExecOutcome::Exited(3), "said\n".to_owned())
    );
    assert!(Utf8PathBuf::from(format!("{stdout}.err")).is_file());
    let deadline = sandbox.now() + Duration::from_secs(1);
    let slow = sandbox.exec(
        &id,
        &argv(&["sleep", "30"]),
        deadline,
        &dir.join("slow.txt"),
    );
    assert_eq!(slow.unwrap(), ExecOutcome::TimedOut);
    sandbox.destroy(&id).unwrap();
}

#[test]
fn fake_exec_writes_stdout_and_times_out() {
    let script = Script {
        probes: vec![
            (
                "said.txt".into(),
                ExecOutcome::Exited(3),
                b"said\n".to_vec(),
            ),
            ("slow.txt".into(), ExecOutcome::TimedOut, Vec::new()),
        ],
        ..Script::default()
    };
    on_fake(
        vec![rule("target", script)],
        exec_writes_stdout_and_times_out,
    );
}

fn leftovers_sees_what_was_made(sandbox: &impl Sandbox) {
    let data = VolumeName::new("data");
    let mut kept = launch("kept", ALPINE);
    kept.command = Some(argv(&["sleep", "600"]));
    kept.mounts = vec![volume(&data, "/data")];
    let id = create(sandbox, kept);
    sandbox.stop(&id).unwrap();
    let snapshot = sandbox.snapshot(&id).unwrap();
    let left = sandbox.leftovers().unwrap();
    assert!(
        left.sandboxes.contains(&id) && left.snapshots.contains(&snapshot),
        "{left:?}"
    );
    sandbox.destroy(&id).unwrap();
    sandbox.drop_snapshot(&snapshot).unwrap();
    sandbox.drop_volumes(&[data]).unwrap();
    let left = sandbox.leftovers().unwrap();
    assert!(
        left.sandboxes.is_empty() && left.volumes.is_empty() && left.snapshots.is_empty(),
        "{left:?}"
    );
}

#[test]
fn fake_leftovers_sees_what_was_made() {
    on_fake(
        vec![rule("kept", Script::default())],
        leftovers_sees_what_was_made,
    );
}

fn pull_of_an_unknown_image_is_not_found(sandbox: &impl Sandbox) {
    let pulled = sandbox.pull(&[ImageRef::new(ALPINE), ImageRef::new(MISSING)]);
    assert!(
        matches!(&pulled, Err(SandboxError::ImageNotFound(image)) if image.as_str() == MISSING),
        "{pulled:?}"
    );
}

#[test]
fn fake_pull_of_an_unknown_image_is_not_found() {
    let fake = Fake::new(Scenario {
        missing_images: vec![MISSING.into()],
        ..Scenario::default()
    });
    pull_of_an_unknown_image_is_not_found(&fake);
}

fn pull_gives_digests_in_order(sandbox: &impl Sandbox) {
    let images = [ImageRef::new(NODE), ImageRef::new(ALPINE)];
    let pulled = sandbox.pull(&images).unwrap();
    let order: Vec<&ImageRef> = pulled.digests.iter().map(|(image, _)| image).collect();
    assert_eq!(order, [&images[0], &images[1]]);
    let digests_given = pulled.digests.iter().all(|(_, digest)| !digest.is_empty());
    assert!(digests_given && !pulled.guest.is_empty(), "{pulled:?}");
}

#[test]
fn fake_pull_gives_digests_in_order() {
    let fake = Fake::new(Scenario {
        digests: vec![(NODE.into(), "sha256:node".into())],
        ..Scenario::default()
    });
    pull_gives_digests_in_order(&fake);
    let pulled = fake
        .pull(&[ImageRef::new(NODE), ImageRef::new(ALPINE)])
        .unwrap();
    let digests: Vec<&str> = pulled
        .digests
        .iter()
        .map(|(_, digest)| digest.as_str())
        .collect();
    assert_eq!(digests, ["sha256:node", "fake:alpine:3.20"]);
    assert_eq!(pulled.guest, "fake");
}

fn a_snapshot_outlives_its_vm(sandbox: &impl Sandbox) {
    let (_first, first) = folder();
    let (_second, second) = folder();
    let mut origin = launch("origin", ALPINE);
    origin.mounts = vec![out(&first)];
    origin.command = sh("echo origin > /root/mark");
    let id = create(sandbox, origin);
    assert_eq!(wait_exit(sandbox, &id), Some(0));
    let snapshot = sandbox.snapshot(&id).unwrap();
    sandbox.destroy(&id).unwrap();
    let mut other = launch("other", ALPINE);
    other.command = Some(argv(&["sleep", "600"]));
    let other = create(sandbox, other);
    sandbox.destroy(&other).unwrap();
    assert!(sandbox.leftovers().unwrap().snapshots.contains(&snapshot));

    let mut copy = launch("copy", ALPINE);
    copy.seed = Some(Seed::Snapshot(snapshot.clone()));
    copy.mounts = vec![out(&second)];
    copy.command = sh("cat /root/mark > /stand/out/mark");
    let copied = create(sandbox, copy.clone());
    assert_eq!(wait_exit(sandbox, &copied), Some(0));
    assert_eq!(read(&second.join("mark")), "origin\n");
    sandbox.destroy(&copied).unwrap();

    sandbox.drop_snapshot(&snapshot).unwrap();
    assert!(!sandbox.leftovers().unwrap().snapshots.contains(&snapshot));
    let refused = sandbox.create(&[copy]);
    assert!(
        matches!(refused, Err(SandboxError::Backend { .. })),
        "from a dropped snapshot: {refused:?}"
    );
}

#[test]
fn fake_a_snapshot_outlives_its_vm() {
    on_fake(
        vec![
            rule("origin", exits(0)),
            rule("other", Script::default()),
            rule("copy", writes(exits(0), &[("/stand/out/mark", "origin\n")])),
        ],
        a_snapshot_outlives_its_vm,
    );
}

#[test]
fn fake_time_moves_only_in_wait_and_a_healthcheck_repeats_its_last_code() {
    let fake = Fake::new(Scenario {
        rules: vec![
            rule(
                "api",
                Script {
                    exit: Some((0, Duration::from_secs(30))),
                    health: vec![1, 0],
                    ..Script::default()
                },
            ),
            rule("db", Script::default()),
        ],
        ..Scenario::default()
    });
    let start = fake.now();
    let ids = fake
        .create(&[launch("api", NODE), launch("db", ALPINE)])
        .unwrap();
    let [api, db] = ids.as_slice() else {
        panic!("{ids:?}")
    };
    let codes: Vec<Option<i32>> = (0..3)
        .map(|_| {
            let check = Waitable::Exec(fake.exec_detached(api, &[]).unwrap());
            fake.wait(&[check], fake.now()).unwrap().unwrap().code
        })
        .collect();
    assert_eq!(codes, [Some(1), Some(0), Some(0)]);
    assert_eq!(fake.now(), start);

    let api_exit = [Waitable::Sandbox(api.clone())];
    assert!(
        fake.wait(&api_exit, start + Duration::from_secs(10))
            .unwrap()
            .is_none()
    );
    assert_eq!(fake.now(), start + Duration::from_secs(10));
    let exited = fake.wait(&api_exit, start + EXIT).unwrap().unwrap();
    assert_eq!(
        (exited.code, exited.at),
        (Some(0), start + Duration::from_secs(30))
    );
    assert_eq!(fake.now(), exited.at);
    let stopped = fake.stop(api).unwrap();
    assert_eq!((stopped.after, stopped.killed), (Duration::ZERO, false));

    let db_exit = [Waitable::Sandbox(db.clone())];
    assert!(
        fake.wait(&db_exit, start + Duration::from_secs(40))
            .unwrap()
            .is_none()
    );
    fake.stop(db).unwrap();
    let both = [db_exit[0].clone(), api_exit[0].clone()];
    let first = fake.wait(&both, fake.now() + EXIT).unwrap().unwrap();
    assert_eq!(
        (first.what, first.code),
        (api_exit[0].clone(), Some(0)),
        "the earlier exit"
    );
    let stopped = fake.wait(&db_exit, fake.now() + EXIT).unwrap().unwrap();
    assert_eq!(
        (stopped.code, stopped.at),
        (None, start + Duration::from_secs(40))
    );
    destroy(&fake, &ids);
}

#[test]
fn fake_metrics_change_under_a_fault_and_the_journal_keeps_every_call() {
    let fake = Fake::new(Scenario {
        rules: vec![rule(
            "api",
            Script {
                metrics: Metrics {
                    cpu: Duration::from_secs(1),
                    ..Metrics::default()
                },
                under_fault: Some(Metrics {
                    cpu: Duration::from_secs(2),
                    ..Metrics::default()
                }),
                ..Script::default()
            },
        )],
        ..Scenario::default()
    });
    let id = fake.create(&[launch("api", NODE)]).unwrap().remove(0);
    assert_eq!(fake.metrics(&id).unwrap().cpu, Duration::from_secs(1));
    let fault = Fault {
        target: "db:5432".into(),
        mode: FaultMode::Down,
    };
    fake.fault(std::slice::from_ref(&id), &fault).unwrap();
    assert_eq!(fake.metrics(&id).unwrap().cpu, Duration::from_secs(2));
    fake.destroy(&id).unwrap();
    assert_gone(fake.fault(std::slice::from_ref(&id), &fault), &id);

    let journal = fake.journal();
    assert!(
        matches!(
            journal.as_slice(),
            [
                Call::Create { ids, .. },
                Call::Metrics(_),
                Call::Fault { .. },
                Call::Metrics(_),
                Call::Destroy(_),
                Call::Fault { .. },
            ] if *ids == [id.clone()]
        ),
        "{journal:#?}"
    );
}

#[test]
fn fake_a_rule_with_a_tree_follows_that_tree_into_its_snapshots() {
    let (_dir, dir) = folder();
    let (head, before) = (dir.join("head.tar"), dir.join("before.tar"));
    let version = |tar: &Utf8Path, who: &str| Rule {
        service: "api".into(),
        tree: Some(tar.to_owned()),
        script: writes(Script::default(), &[("/stand/out/who", who)]),
    };
    let fake = Fake::new(Scenario {
        rules: vec![version(&head, "head\n"), version(&before, "before\n")],
        ..Scenario::default()
    });
    let [at_head, restored_dir, at_before] =
        ["at-head", "restored", "at-before"].map(|name| dir.join(name));
    for bound in [&at_head, &restored_dir, &at_before] {
        fs::create_dir(bound).unwrap();
    }
    let mut api = launch("api", NODE);
    api.seed = Some(Seed::Tree {
        tar: head.clone(),
        at: "/w/app".into(),
    });
    api.mounts = vec![out(&at_head)];
    let first = fake.create(std::slice::from_ref(&api)).unwrap().remove(0);
    assert_eq!(read(&at_head.join("who")), "head\n");
    fake.stop(&first).unwrap();
    let snapshot = fake.snapshot(&first).unwrap();
    fake.destroy(&first).unwrap();

    api.seed = Some(Seed::Snapshot(snapshot.clone()));
    api.mounts = vec![out(&restored_dir)];
    let restored = fake.create(&[api.clone()]).unwrap().remove(0);
    assert_eq!(read(&restored_dir.join("who")), "head\n");

    api.seed = Some(Seed::Tree {
        tar: before,
        at: "/w/app".into(),
    });
    api.mounts = vec![out(&at_before)];
    let other = fake.create(&[api]).unwrap().remove(0);
    assert_eq!(read(&at_before.join("who")), "before\n");
    destroy(&fake, &[restored, other]);
    fake.drop_snapshot(&snapshot).unwrap();
}

/// Runs a shared case on its own `Fake` and checks it left nothing, as the live harness does.
fn on_fake(rules: Vec<Rule>, case: impl FnOnce(&Fake)) {
    let fake = Fake::new(Scenario {
        rules,
        ..Scenario::default()
    });
    case(&fake);
    let left = fake.leftovers().unwrap();
    assert!(
        left.sandboxes.is_empty() && left.volumes.is_empty() && left.snapshots.is_empty(),
        "left behind: {left:?}"
    );
}

fn rule(service: &str, script: Script) -> Rule {
    Rule {
        service: service.into(),
        tree: None,
        script,
    }
}

fn exits(code: i32) -> Script {
    Script {
        exit: Some((code, Duration::from_secs(1))),
        ..Script::default()
    }
}

fn writes(script: Script, files: &[(&str, &str)]) -> Script {
    let writes = files
        .iter()
        .map(|(path, text)| (path.to_string(), text.as_bytes().to_vec()));
    Script {
        writes: writes.collect(),
        ..script
    }
}

fn launch(service: &str, image: &str) -> Launch {
    Launch {
        service: service.into(),
        replica: 1,
        image: ImageRef::new(image),
        cpus: 1,
        memory: 512 << 20,
        env: BTreeMap::new(),
        workdir: None,
        entrypoint: None,
        command: None,
        mounts: Vec::new(),
        ports: Vec::new(),
        peers: Vec::new(),
        outbound: Outbound::Off,
        stop_signal: "SIGTERM".into(),
        stop_grace: Duration::from_secs(5),
        max_duration: Duration::from_secs(15 * 60),
        seed: None,
    }
}

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

fn sh(script: &str) -> Option<Vec<String>> {
    Some(argv(&["sh", "-c", script]))
}

/// Where a service writes what the case reads, and where a probe's stdout must lie.
fn out(dir: &Utf8Path) -> Mount {
    Mount::Bind {
        host: dir.to_owned(),
        guest: "/stand/out".into(),
        read_only: false,
    }
}

fn volume(name: &VolumeName, guest: &str) -> Mount {
    Mount::Volume {
        name: name.clone(),
        guest: guest.into(),
    }
}

fn folder() -> (TempDir, Utf8PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let path = Utf8Path::from_path(temp.path()).unwrap().to_owned();
    (temp, path)
}

fn create(sandbox: &impl Sandbox, launch: Launch) -> SandboxId {
    sandbox.create(&[launch]).unwrap().remove(0)
}

fn destroy(sandbox: &impl Sandbox, ids: &[SandboxId]) {
    for id in ids {
        sandbox.destroy(id).unwrap();
    }
}

fn wait_exit(sandbox: &impl Sandbox, id: &SandboxId) -> Option<i32> {
    let what = [Waitable::Sandbox(id.clone())];
    let exited = sandbox.wait(&what, sandbox.now() + EXIT).unwrap();
    exited.expect("the service exits").code
}

fn assert_gone<T: Debug>(result: Result<T, SandboxError>, id: &SandboxId) {
    assert!(
        matches!(&result, Err(SandboxError::Gone(gone)) if gone == id),
        "{result:?}"
    );
}

/// Sorted, since the order of the volumes is the adapter's.
fn names(sizes: &[(VolumeName, u64)]) -> Vec<VolumeName> {
    let mut names: Vec<VolumeName> = sizes.iter().map(|(name, _)| name.clone()).collect();
    names.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    names
}

/// `<who> <vCPUs> <MiB>`, as `LIMITS` writes it.
fn limits(seen: &str) -> (String, u32, u64) {
    let words: Vec<&str> = seen.split_whitespace().collect();
    let [who, cpus, mib] = words.as_slice() else {
        panic!("not `<who> <vCPUs> <MiB>`: {seen:?}");
    };
    (who.to_string(), cpus.parse().unwrap(), mib.parse().unwrap())
}

fn read(path: &Utf8Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// An executable script, a file above 4 MiB and a symlink to the script: the entry kinds `source::archive` writes.
fn write_tree(path: &Utf8Path) {
    let mut tar = tar::Builder::new(fs::File::create(path).unwrap());
    let mut file = |name: &str, mode: u32, bytes: &[u8]| {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(mode);
        header.set_cksum();
        tar.append_data(&mut header, name, bytes).unwrap();
    };
    file("run.sh", 0o755, RUN_SH.as_bytes());
    file("big.bin", 0o644, &vec![b'x'; BIG]);
    let mut link = tar::Header::new_gnu();
    link.set_entry_type(tar::EntryType::Symlink);
    link.set_size(0);
    link.set_mode(0o777);
    tar.append_link(&mut link, "link", "run.sh").unwrap();
    tar.finish().unwrap();
}

/// The shared cases on `Microsandbox`, and the cases only a real guest shows. Run with
/// `STOP_SLOP_STAND_LIVE=1` (and `STOP_SLOP_STAND_GUEST=<musl build>` before the pin)
/// `cargo test --locked -p sandbox --features microsandbox --test contract -- --test-threads=1`:
/// one case at a time, since `leftovers` sees the whole namespace.
#[cfg(feature = "microsandbox")]
mod live {
    use std::io::Write as _;
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    use camino::Utf8PathBuf;
    use sandbox::{ImageRef, Launch, Microsandbox, Sandbox, SandboxId};

    use super::{ALPINE, EXIT, NGINX, NODE, launch, sh, wait_exit};

    const POSTGRES: &str = "postgres:17-alpine";

    fn live() -> Option<&'static Microsandbox> {
        if std::env::var("STOP_SLOP_STAND_LIVE").as_deref() != Ok("1") {
            return None;
        }
        static LIVE: OnceLock<Microsandbox> = OnceLock::new();
        Some(LIVE.get_or_init(|| {
            let _ = std::fs::remove_file(home().join("notes.txt"));
            let sandbox = Microsandbox::new(&home()).expect("the adapter");
            let pulled = sandbox.pull(&images(&[NODE, POSTGRES, NGINX]));
            pulled.expect("pull the images of the live set");
            sweep(&sandbox);
            sandbox
        }))
    }

    /// One home for every live run, kept between runs, so the runtime and the images download
    /// once. On Linux and macOS it is under `/tmp`: the SDK keeps Unix sockets under its home, and
    /// the per-user temp folder of macOS is too deep for `sun_path`.
    fn home() -> Utf8PathBuf {
        if cfg!(unix) {
            Utf8PathBuf::from("/tmp/stop-slop-stand-live")
        } else {
            let temp = Utf8PathBuf::from_path_buf(std::env::temp_dir()).unwrap();
            temp.join("stop-slop-stand-live")
        }
    }

    /// What a crashed run or a panicked case left.
    fn sweep(sandbox: &Microsandbox) {
        let left = sandbox.leftovers().expect("leftovers");
        for id in &left.sandboxes {
            sandbox.destroy(id).expect("destroy a leftover");
        }
        for snapshot in &left.snapshots {
            sandbox
                .drop_snapshot(snapshot)
                .expect("drop a leftover snapshot");
        }
    }

    /// One live case: a sweep when it panics, and at its end the check that it left nothing.
    struct Case {
        sandbox: &'static Microsandbox,
    }

    impl Case {
        fn start() -> Option<Case> {
            live().map(|sandbox| Case { sandbox })
        }

        fn finish(self, created: &[SandboxId]) {
            for id in created {
                self.sandbox.destroy(id).unwrap();
            }
            let left = self.sandbox.leftovers().unwrap();
            assert!(
                left.sandboxes.is_empty() && left.snapshots.is_empty(),
                "left behind: {left:?}"
            );
        }

        fn create(&self, batch: Vec<Launch>) -> Vec<SandboxId> {
            self.sandbox.create(&batch).expect("create")
        }

        fn log(&self, id: &SandboxId) -> String {
            String::from_utf8_lossy(&self.sandbox.logs(id).unwrap()).into_owned()
        }

        fn until_logged(&self, id: &SandboxId, line: &str) {
            let deadline = Instant::now() + EXIT;
            while !self.log(id).lines().any(|logged| logged == line) {
                assert!(Instant::now() < deadline, "no `{line}` in the log of {id}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    impl Drop for Case {
        fn drop(&mut self) {
            if std::thread::panicking() {
                sweep(self.sandbox);
            }
        }
    }

    fn images(names: &[&str]) -> Vec<ImageRef> {
        names.iter().map(|name| ImageRef::new(*name)).collect()
    }

    /// Lines for the PR description, beside the live home.
    fn note(line: &str) {
        let path = home().join("notes.txt");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
        writeln!(file, "{os} {arch}: {line}").unwrap();
    }

    macro_rules! shared {
        ($($case:ident),* $(,)?) => {$(
            #[test]
            fn $case() {
                let Some(case) = Case::start() else { return };
                super::$case(case.sandbox);
                case.finish(&[]);
            }
        )*};
    }

    shared!(
        create_batch_and_wait,
        logs_and_metrics_until_destroy,
        pull_of_an_unknown_image_is_not_found,
        pull_gives_digests_in_order,
    );

    /// Row 88, a stop rule of T-11: without a digest the setup cache key cannot see a new image
    /// under the same tag.
    #[test]
    fn live_pull_gives_a_digest_per_image_and_the_guest_version() {
        let Some(case) = Case::start() else { return };
        let pulled = case.sandbox.pull(&images(&[NODE, POSTGRES])).unwrap();
        let names: Vec<&str> = pulled
            .digests
            .iter()
            .map(|(image, _)| image.as_str())
            .collect();
        assert_eq!(names, [NODE, POSTGRES]);
        let digests = pulled.digests.iter().map(|(_, digest)| digest);
        assert!(
            digests.clone().all(|digest| digest.starts_with("sha256:")),
            "{pulled:?}"
        );
        let dev = std::env::var_os("STOP_SLOP_STAND_GUEST").is_some();
        assert_eq!(pulled.guest.starts_with("dev-"), dev, "{pulled:?}");
        note(&format!(
            "pull: {:?}, guest {}",
            pulled.digests, pulled.guest
        ));
        case.finish(&[]);
    }

    /// Row 72, a stop rule of T-11: the signal of `stand-guest stop` must reach the service, also a
    /// `SIGQUIT` (the stop signal of nginx images). An ignored signal survives `exec`, so no command
    /// of the service may start with one. Each service logs `ready` once its trap is set, the clean
    /// one on stderr and after the signals ignored or blocked in `run`, as the agent starts it, and
    /// in a command of the service; not in the shell itself, since busybox `sh` ignores `SIGQUIT` on
    /// its own and blocks every signal while it waits for a child.
    #[test]
    fn live_stop_signals_the_service_and_kills_it_at_grace() {
        let Some(case) = Case::start() else { return };
        let mut clean = launch("clean", ALPINE);
        clean.command = sh(
            "awk '/^Sig(Ign|Blk)/ {print \"run\", $0}' /proc/$PPID/status; \
             awk '/^Sig(Ign|Blk)/ {print \"service\", $0}' /proc/self/status; \
             trap 'exit 0' TERM; echo ready >&2; while true; do sleep 0.1; done",
        );
        let mut quitter = launch("quitter", ALPINE);
        quitter.command = sh("trap 'exit 0' QUIT; echo ready; while true; do sleep 0.1; done");
        quitter.stop_signal = "SIGQUIT".into();
        let mut stubborn = launch("stubborn", ALPINE);
        stubborn.command = sh("trap '' TERM; echo ready; while true; do sleep 0.1; done");
        stubborn.stop_grace = Duration::from_secs(2);
        let ids = case.create(vec![clean, quitter, stubborn]);
        for id in &ids {
            case.until_logged(id, "ready");
        }
        let log = case.log(&ids[0]);
        let signals: Vec<&str> = log.lines().filter(|line| line.contains(" Sig")).collect();
        note(&format!("signals ignored and blocked: {signals:?}"));
        let ignored = "service SigIgn:\t0000000000000000";
        assert!(signals.contains(&ignored), "{signals:?}");

        for id in &ids[..2] {
            let stopped = case.sandbox.stop(id).unwrap();
            let fast = stopped.after < Duration::from_secs(2);
            assert!(!stopped.killed && fast, "{id}: {stopped:?}");
            assert_eq!(wait_exit(case.sandbox, id), Some(0), "{id}: the trap ran");
        }
        let killed = case.sandbox.stop(&ids[2]).unwrap();
        let at_grace = Duration::from_millis(1900)..Duration::from_secs(4);
        assert!(
            killed.killed && at_grace.contains(&killed.after),
            "{killed:?}"
        );
        assert_eq!(wait_exit(case.sandbox, &ids[2]), Some(128 + 9));
        case.finish(&ids);
    }
}
