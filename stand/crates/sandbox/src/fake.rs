//! A scripted `Sandbox` for the tests of `run` and `cli`: each VM follows the `Script` of its rule,
//! time moves only in `wait`, and every call lands in a journal. It refuses what microsandbox
//! refuses, with the adapter's variant and words (the docs of `create`, `exec`, `Seed::Snapshot`),
//! so a test of `run` catches such a call before a live run does.

use std::fs;
use std::io;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};

use crate::{
    ExecId, ExecOutcome, Exited, Exposed, Fault, ImageRef, Launch, Leftovers, Metrics, Mount,
    Pulled, Sandbox, SandboxError, SandboxId, Seed, SnapshotId, Stopped, VolumeName, Waitable,
};

pub struct Fake {
    scenario: Scenario,
    state: Mutex<State>,
}

/// The first rule whose `service` and, when set, `tree` match wins. `tree` is the tar of `Seed::Tree`, or of the
/// VM whose snapshot seeds this one.
/// `digests`: what `pull` reports per image; an unlisted image gets one derived from its name. A snapshot lives
/// until `drop_snapshot`, across any number of runs on one `Fake`, and `guest` of `Pulled` is `fake`.
#[derive(Debug, Clone, Default)]
pub struct Scenario {
    pub rules: Vec<Rule>,
    pub missing_images: Vec<String>,
    pub digests: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub service: String,
    pub tree: Option<Utf8PathBuf>,
    pub script: Script,
}

#[derive(Debug, Clone, Default)]
pub struct Script {
    /// Exit code and the time after `create`; `None` runs until `destroy`.
    pub exit: Option<(i32, Duration)>,
    /// How successive healthcheck attempts end, the last one repeating: with a code the moment they
    /// start, or with `None` at their `timeout`, as a hung attempt the guest kills there.
    pub health: Vec<Option<i32>>,
    /// By the file name of the `stdout` of an `exec` (`probe.orders.jsonl`): its outcome and bytes; an unscripted
    /// `exec` exits 0 with nothing.
    pub probes: Vec<(String, ExecOutcome, Vec<u8>)>,
    /// What `stop` reports; the service then ends with 0, as a clean shutdown, or with 137 when
    /// `killed` at the grace.
    pub stop: Stopped,
    pub metrics: Metrics,
    /// `metrics` after `fault`: what the relays saw under it.
    pub under_fault: Option<Metrics>,
    pub volume_bytes: u64,
    /// Files written on `create` at guest paths: under a bind into its host folder, under a `Scratch` folder
    /// kept for `copy_out`.
    pub writes: Vec<(String, Vec<u8>)>,
}

/// A port call with its arguments; `Create` and `Snapshot` also with what they made, nothing when refused.
#[derive(Debug, Clone)]
pub enum Call {
    Pull(Vec<ImageRef>),
    Create {
        batch: Vec<Launch>,
        ids: Vec<SandboxId>,
    },
    ExecDetached {
        id: SandboxId,
        argv: Vec<String>,
        timeout: Duration,
    },
    Wait {
        what: Vec<Waitable>,
        deadline: Instant,
    },
    Exec {
        id: SandboxId,
        argv: Vec<String>,
        deadline: Instant,
        stdout: Utf8PathBuf,
    },
    Stop(SandboxId),
    Fault {
        ids: Vec<SandboxId>,
        fault: Fault,
    },
    Metrics(SandboxId),
    Logs(SandboxId),
    CopyOut {
        id: SandboxId,
        guest: String,
        host: Utf8PathBuf,
    },
    VolumeSizes(SandboxId),
    Snapshot {
        id: SandboxId,
        snapshot: Option<SnapshotId>,
    },
    Destroy(SandboxId),
    DropSnapshot(SnapshotId),
    Leftovers,
    PruneImages(Vec<ImageRef>),
}

struct State {
    clock: Instant,
    made: u64,
    vms: Vec<Vm>,
    execs: Vec<Job>,
    snapshots: Vec<Snap>,
    journal: Vec<Call>,
}

struct Vm {
    id: SandboxId,
    launch: Launch,
    script: Script,
    tree: Option<Utf8PathBuf>,
    /// The volumes a snapshot of it carries: its own, or those of the snapshot it came from.
    disks: Vec<(VolumeName, String)>,
    started: Instant,
    stopped: Option<Instant>,
    healthchecks: usize,
    faulted: bool,
    /// `writes` under a `Scratch` folder, by guest path.
    scratch: Vec<(String, Vec<u8>)>,
}

/// A healthcheck attempt.
struct Job {
    id: ExecId,
    vm: SandboxId,
    at: Instant,
    code: Option<i32>,
}

struct Snap {
    id: SnapshotId,
    binds: Vec<String>,
    disks: Vec<(VolumeName, String)>,
    tree: Option<Utf8PathBuf>,
}

impl Fake {
    pub fn new(scenario: Scenario) -> Fake {
        let state = State {
            clock: Instant::now(),
            made: 0,
            vms: Vec::new(),
            execs: Vec::new(),
            snapshots: Vec::new(),
            journal: Vec::new(),
        };
        Fake {
            scenario,
            state: Mutex::new(state),
        }
    }

    /// Every port call but `now`, in order.
    pub fn journal(&self) -> Vec<Call> {
        self.state().journal.clone()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .expect("an earlier call of the fake panicked")
    }

    /// `create` and `snapshot` journal after the work instead, with what they made.
    fn record(&self, call: Call) -> MutexGuard<'_, State> {
        let mut state = self.state();
        state.journal.push(call);
        state
    }

    /// The whole batch is checked before anything is made, so a refused batch leaves nothing.
    fn make(&self, state: &mut State, batch: &[Launch]) -> Result<Vec<SandboxId>, SandboxError> {
        let mut vms = Vec::with_capacity(batch.len());
        for launch in batch {
            if self.is_missing(&launch.image) {
                return Err(SandboxError::ImageNotFound(launch.image.clone()));
            }
            if let Some((host, guest)) = missing_bind(launch) {
                return Err(backend(format!(
                    "{host}, bound on {guest} for `{}`, is not a folder on the host",
                    launch.service
                )));
            }
            if let Some((vm, port)) = state.port_holder(launch) {
                return Err(backend(format!(
                    "host port of `{}` replica {} port {port} is in use: by the live VM `{}` of \
                     that replica",
                    launch.service, launch.replica, vm.id
                )));
            }
            let (tree, disks) = match &launch.seed {
                None => (None, volumes(launch)),
                Some(Seed::Tree { tar, .. }) => (Some(tar.clone()), volumes(launch)),
                Some(Seed::Snapshot(snapshot)) => {
                    let snap = state.snap(snapshot)?;
                    restorable(snap, launch)?;
                    (snap.tree.clone(), snap.disks.clone())
                }
            };
            let script = self.script(&launch.service, tree.as_ref()).clone();
            let n = state.next();
            vms.push(Vm {
                id: SandboxId::new(format!("fake-{n}-{}-{}", launch.service, launch.replica)),
                launch: launch.clone(),
                script,
                tree,
                disks,
                started: state.clock,
                stopped: None,
                healthchecks: 0,
                faulted: false,
                scratch: Vec::new(),
            });
        }
        for vm in &mut vms {
            vm.write_files()?;
        }
        let ids = vms.iter().map(|vm| vm.id.clone()).collect();
        state.vms.extend(vms);
        Ok(ids)
    }

    fn script(&self, service: &str, tree: Option<&Utf8PathBuf>) -> &Script {
        let rule = self.scenario.rules.iter().find(|rule| {
            rule.service == service && rule.tree.as_ref().is_none_or(|own| Some(own) == tree)
        });
        match rule {
            Some(rule) => &rule.script,
            None => panic!("the scenario has no rule for `{service}` with the tree {tree:?}"),
        }
    }

    fn is_missing(&self, image: &ImageRef) -> bool {
        let missing = &self.scenario.missing_images;
        missing.iter().any(|name| name == image.as_str())
    }

    fn digest(&self, image: &ImageRef) -> String {
        let listed = self
            .scenario
            .digests
            .iter()
            .find(|(name, _)| name == image.as_str());
        match listed {
            Some((_, digest)) => digest.clone(),
            None => format!("fake:{image}"),
        }
    }
}

impl Sandbox for Fake {
    fn now(&self) -> Instant {
        self.state().clock
    }

    fn pull(&self, images: &[ImageRef]) -> Result<Pulled, SandboxError> {
        drop(self.record(Call::Pull(images.to_vec())));
        if let Some(image) = images.iter().find(|image| self.is_missing(image)) {
            return Err(SandboxError::ImageNotFound(image.clone()));
        }
        Ok(Pulled {
            digests: images
                .iter()
                .map(|image| (image.clone(), self.digest(image)))
                .collect(),
            guest: "fake".to_owned(),
        })
    }

    fn create(&self, batch: &[Launch]) -> Result<Vec<SandboxId>, SandboxError> {
        let mut state = self.state();
        let made = self.make(&mut state, batch);
        state.journal.push(Call::Create {
            batch: batch.to_vec(),
            ids: made.as_ref().cloned().unwrap_or_default(),
        });
        made
    }

    fn exec_detached(
        &self,
        id: &SandboxId,
        argv: &[String],
        timeout: Duration,
    ) -> Result<ExecId, SandboxError> {
        let mut state = self.record(Call::ExecDetached {
            id: id.clone(),
            argv: argv.to_vec(),
            timeout,
        });
        let clock = state.clock;
        let vm = state.vm_mut(id)?;
        let health = &vm.script.health;
        let code = health
            .get(vm.healthchecks)
            .or(health.last())
            .copied()
            .unwrap_or(Some(0));
        vm.healthchecks += 1;
        let at = if code.is_some() {
            clock
        } else {
            clock + timeout
        };
        let exec = ExecId::new(format!("exec-{}", state.next()));
        state.execs.push(Job {
            id: exec.clone(),
            vm: id.clone(),
            at,
            code,
        });
        Ok(exec)
    }

    fn wait(&self, what: &[Waitable], deadline: Instant) -> Result<Option<Exited>, SandboxError> {
        let mut state = self.record(Call::Wait {
            what: what.to_vec(),
            deadline,
        });
        let mut first: Option<Exited> = None;
        for waitable in what {
            let end = match waitable {
                Waitable::Sandbox(id) => state.vm(id)?.ended(),
                Waitable::Exec(exec) => {
                    let job = state.job(exec);
                    state.vm(&job.vm)?;
                    Some((job.at, job.code))
                }
            };
            if let Some((at, code)) = end
                && first.as_ref().is_none_or(|first| at < first.at)
            {
                first = Some(Exited {
                    what: waitable.clone(),
                    code,
                    at,
                });
            }
        }
        let exited = first.filter(|exited| exited.at <= deadline);
        let until = exited.as_ref().map_or(deadline, |exited| exited.at);
        state.clock = state.clock.max(until);
        Ok(exited)
    }

    fn exec(
        &self,
        id: &SandboxId,
        argv: &[String],
        deadline: Instant,
        stdout: &Utf8Path,
    ) -> Result<ExecOutcome, SandboxError> {
        let state = self.record(Call::Exec {
            id: id.clone(),
            argv: argv.to_vec(),
            deadline,
            stdout: stdout.to_owned(),
        });
        let vm = state.vm(id)?;
        if !under_writable_bind(&vm.launch, stdout) {
            return Err(SandboxError::Guest {
                message: format!("{stdout} is not a file under a writable bind of the VM"),
            });
        }
        let name = stdout.file_name().unwrap_or_default();
        let scripted = vm.script.probes.iter().find(|(file, ..)| file == name);
        let (outcome, bytes) = match scripted {
            Some((_, outcome, bytes)) => (*outcome, bytes.as_slice()),
            None => (ExecOutcome::Exited(0), [].as_slice()),
        };
        // No folders made: the guest would not make them either.
        fs::write(stdout, bytes).map_err(io_error(stdout))?;
        let stderr = Utf8PathBuf::from(format!("{stdout}.err"));
        fs::write(&stderr, b"").map_err(io_error(&stderr))?;
        Ok(outcome)
    }

    fn stop(&self, id: &SandboxId) -> Result<Stopped, SandboxError> {
        let mut state = self.record(Call::Stop(id.clone()));
        let clock = state.clock;
        let vm = state.vm_mut(id)?;
        // As `stand-guest stop` reports a command that ended before it.
        if vm.ended().is_some_and(|(at, _)| at <= clock) {
            return Ok(Stopped::default());
        }
        vm.stopped = Some(clock);
        Ok(vm.script.stop)
    }

    fn fault(&self, ids: &[SandboxId], fault: &Fault) -> Result<(), SandboxError> {
        let mut state = self.record(Call::Fault {
            ids: ids.to_vec(),
            fault: fault.clone(),
        });
        for id in ids {
            state.vm(id)?;
        }
        for id in ids {
            state.vm_mut(id)?.faulted = true;
        }
        Ok(())
    }

    fn metrics(&self, id: &SandboxId) -> Result<Metrics, SandboxError> {
        let state = self.record(Call::Metrics(id.clone()));
        let vm = state.vm(id)?;
        match &vm.script.under_fault {
            Some(under_fault) if vm.faulted => Ok(under_fault.clone()),
            _ => Ok(vm.script.metrics.clone()),
        }
    }

    fn logs(&self, id: &SandboxId) -> Result<Vec<u8>, SandboxError> {
        self.record(Call::Logs(id.clone())).vm(id)?;
        Ok(Vec::new())
    }

    fn copy_out(&self, id: &SandboxId, guest: &str, host: &Utf8Path) -> Result<(), SandboxError> {
        let state = self.record(Call::CopyOut {
            id: id.clone(),
            guest: guest.to_owned(),
            host: host.to_owned(),
        });
        let vm = state.vm(id)?;
        if fs::read_dir(host).map_err(io_error(host))?.next().is_some() {
            return Err(backend(format!(
                "fake: {host}: not empty, copy_out takes an empty folder"
            )));
        }
        if !scratch_holds(&vm.launch, guest) {
            return Err(backend(format!(
                "fake: `{guest}` is not in a Scratch folder of `{id}`, and only those hold files"
            )));
        }
        for (path, bytes) in &vm.scratch {
            if let Some(rest) = under(path, guest).filter(|rest| !rest.is_empty()) {
                write_file(&host.join(rest), bytes)?;
            }
        }
        Ok(())
    }

    fn volume_sizes(&self, id: &SandboxId) -> Result<Vec<(VolumeName, u64)>, SandboxError> {
        let state = self.record(Call::VolumeSizes(id.clone()));
        let vm = state.vm(id)?;
        let sizes = volumes(&vm.launch)
            .into_iter()
            .map(|(name, _)| (name, vm.script.volume_bytes));
        Ok(sizes.collect())
    }

    fn snapshot(&self, id: &SandboxId) -> Result<SnapshotId, SandboxError> {
        let mut state = self.state();
        let taken = state.take_snapshot(id);
        state.journal.push(Call::Snapshot {
            id: id.clone(),
            snapshot: taken.as_ref().ok().cloned(),
        });
        taken
    }

    fn destroy(&self, id: &SandboxId) -> Result<(), SandboxError> {
        let mut state = self.record(Call::Destroy(id.clone()));
        state.vms.retain(|vm| vm.id != *id);
        Ok(())
    }

    fn drop_snapshot(&self, snapshot: &SnapshotId) -> Result<(), SandboxError> {
        let mut state = self.record(Call::DropSnapshot(snapshot.clone()));
        state.snapshots.retain(|snap| snap.id != *snapshot);
        Ok(())
    }

    fn leftovers(&self) -> Result<Leftovers, SandboxError> {
        let state = self.record(Call::Leftovers);
        Ok(Leftovers {
            sandboxes: state.vms.iter().map(|vm| vm.id.clone()).collect(),
            snapshots: state.snapshots.iter().map(|snap| snap.id.clone()).collect(),
        })
    }

    fn prune_images(&self, keep: &[ImageRef]) -> Result<(), SandboxError> {
        drop(self.record(Call::PruneImages(keep.to_vec())));
        Ok(())
    }
}

impl State {
    fn next(&mut self) -> u64 {
        self.made += 1;
        self.made
    }

    fn vm(&self, id: &SandboxId) -> Result<&Vm, SandboxError> {
        let vm = self.vms.iter().find(|vm| vm.id == *id);
        vm.ok_or_else(|| SandboxError::Gone(id.clone()))
    }

    fn vm_mut(&mut self, id: &SandboxId) -> Result<&mut Vm, SandboxError> {
        let vm = self.vms.iter_mut().find(|vm| vm.id == *id);
        vm.ok_or_else(|| SandboxError::Gone(id.clone()))
    }

    fn port_holder(&self, launch: &Launch) -> Option<(&Vm, u16)> {
        let replica =
            |vm: &&Vm| vm.launch.service == launch.service && vm.launch.replica == launch.replica;
        self.vms.iter().filter(replica).find_map(|vm| {
            let common = launch
                .ports
                .iter()
                .find(|port| vm.launch.ports.contains(port));
            common.map(|&Exposed(port)| (vm, port))
        })
    }

    fn job(&self, exec: &ExecId) -> &Job {
        let job = self.execs.iter().find(|job| job.id == *exec);
        job.expect("an exec id this fake gave")
    }

    fn snap(&self, snapshot: &SnapshotId) -> Result<&Snap, SandboxError> {
        let snap = self.snapshots.iter().find(|snap| snap.id == *snapshot);
        snap.ok_or_else(|| backend(format!("fake: no snapshot `{snapshot}`")))
    }

    fn take_snapshot(&mut self, id: &SandboxId) -> Result<SnapshotId, SandboxError> {
        let vm = self.vm(id)?;
        let binds: Vec<String> = vm
            .launch
            .mounts
            .iter()
            .filter_map(|mount| match mount {
                Mount::Bind { guest, .. } => Some(guest.clone()),
                _ => None,
            })
            .collect();
        let (disks, tree) = (vm.disks.clone(), vm.tree.clone());
        let snapshot = SnapshotId::new(format!("snapshot-{}", self.next()));
        self.snapshots.push(Snap {
            id: snapshot.clone(),
            binds,
            disks,
            tree,
        });
        Ok(snapshot)
    }
}

impl Vm {
    /// When the service ended and its code: the scripted exit, or `stop` when it came first.
    fn ended(&self) -> Option<(Instant, Option<i32>)> {
        let exit = self
            .script
            .exit
            .map(|(code, after)| (self.started + after, Some(code)));
        let stop_code = if self.script.stop.killed { 128 + 9 } else { 0 };
        match (exit, self.stopped) {
            (Some(exit), Some(stopped)) if exit.0 <= stopped => Some(exit),
            (_, Some(stopped)) => Some((stopped, Some(stop_code))),
            (exit, None) => exit,
        }
    }

    /// A write under neither a writable bind nor a `Scratch` folder is a mistake in the scenario.
    fn write_files(&mut self) -> Result<(), SandboxError> {
        for (path, bytes) in &self.script.writes {
            if let Some(file) = host_file(&self.launch, path) {
                write_file(&file, bytes)?;
            } else if scratch_holds(&self.launch, path) {
                self.scratch.push((path.clone(), bytes.clone()));
            } else {
                panic!(
                    "the script of `{}` writes `{path}`, under no writable bind and no Scratch folder",
                    self.launch.service
                );
            }
        }
        Ok(())
    }
}

/// A restore binds every path the snapshot's VM bound and adds no volume, as microsandbox requires.
fn restorable(snap: &Snap, launch: &Launch) -> Result<(), SandboxError> {
    for path in &snap.binds {
        let bound = launch
            .mounts
            .iter()
            .any(|mount| matches!(mount, Mount::Bind { guest, .. } if guest == path));
        if !bound {
            return Err(backend(format!(
                "the VM of snapshot `{}` bound `{path}`, and the launch of `{}` binds nothing there",
                snap.id, launch.service
            )));
        }
    }
    // By guest path, as microsandbox's snapshot knows its disks.
    for (name, guest) in volumes(launch) {
        if !snap.disks.iter().any(|(_, carried)| *carried == guest) {
            return Err(backend(format!(
                "snapshot `{}` carries no volume at `{guest}` for `{name}`, and a restore adds none",
                snap.id
            )));
        }
    }
    Ok(())
}

fn volumes(launch: &Launch) -> Vec<(VolumeName, String)> {
    let volumes = launch.mounts.iter().filter_map(|mount| match mount {
        Mount::Volume { name, guest } => Some((name.clone(), guest.clone())),
        _ => None,
    });
    volumes.collect()
}

fn missing_bind(launch: &Launch) -> Option<(&Utf8Path, &str)> {
    launch.mounts.iter().find_map(|mount| match mount {
        Mount::Bind { host, guest, .. } if !host.is_dir() => Some((host.as_path(), guest.as_str())),
        _ => None,
    })
}

/// As the adapter maps `stdout` into the guest: below the host folder of a writable bind, by names
/// only, so neither the folder itself nor a path with `..` passes.
fn under_writable_bind(launch: &Launch, stdout: &Utf8Path) -> bool {
    launch.mounts.iter().any(|mount| match mount {
        Mount::Bind {
            host,
            read_only: false,
            ..
        } => stdout.strip_prefix(host).is_ok_and(|rest| {
            let mut parts = rest.components();
            !rest.as_str().is_empty() && parts.all(|part| matches!(part, Utf8Component::Normal(_)))
        }),
        _ => false,
    })
}

fn host_file(launch: &Launch, path: &str) -> Option<Utf8PathBuf> {
    launch.mounts.iter().find_map(|mount| match mount {
        Mount::Bind {
            host,
            guest,
            read_only: false,
        } => under(path, guest).map(|rest| host.join(rest)),
        _ => None,
    })
}

fn scratch_holds(launch: &Launch, path: &str) -> bool {
    launch
        .mounts
        .iter()
        .any(|mount| matches!(mount, Mount::Scratch { guest } if under(path, guest).is_some()))
}

/// The part of the guest path `path` below the guest folder `folder`: `Some("")` for the folder
/// itself, `None` outside it (`/stand/outer` is not in `/stand/out`).
fn under<'a>(path: &'a str, folder: &str) -> Option<&'a str> {
    match path.strip_prefix(folder.trim_end_matches('/'))? {
        "" => Some(""),
        rest => rest.strip_prefix('/'),
    }
}

fn write_file(path: &Utf8Path, bytes: &[u8]) -> Result<(), SandboxError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_error(parent))?;
    }
    fs::write(path, bytes).map_err(io_error(path))
}

fn io_error(path: &Utf8Path) -> impl FnOnce(io::Error) -> SandboxError + '_ {
    move |error| backend(format!("fake: {path}: {error}"))
}

fn backend(message: String) -> SandboxError {
    SandboxError::Backend { message }
}
