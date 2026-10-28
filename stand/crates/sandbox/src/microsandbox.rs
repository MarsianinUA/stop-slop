//! The port on microsandbox 0.7.8: one VM per `Launch`, its service under `stand-guest run`.
//!
//! The SDK is async; each port call blocks on a private runtime and runs its future as a task on a
//! worker with a 16 MiB stack, since the SDK's futures overflow the 1 MiB main thread of a Windows
//! debug build. `home` is microsandbox's own home (runtime, images, VMs, snapshots) and holds the
//! adapter's `vm/<vm>/` (the host side of the VM's `/stand/run` and `/stand/net`, and the tar of
//! `copy_out` on its way) and `guest/<version>/` (`/stand/bin`).

mod guest;
mod network;
mod process;
mod volumes;

use std::collections::BTreeMap;
use std::fmt::Display;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ::microsandbox as msb;
use camino::{Utf8Path, Utf8PathBuf};
use msb::jobs::{Job, JobLogOptions, JobState};
use msb::sandbox::DestroyOptions;
use tokio::sync::OnceCell;
use tokio::time::timeout;

use crate::{
    ExecId, ExecOutcome, Exited, Fault, ImageRef, Launch, Leftovers, Metrics, Mount, Pulled,
    Sandbox, SandboxError, SandboxId, Seed, SnapshotId, Stopped, VolumeName, Waitable,
};

/// The prefix of every VM and snapshot name, and the label key where the SDK takes one: a VM
/// restored from a snapshot gets no label (`RestoreBuilder` has no setter), so `leftovers` goes by
/// name.
const NAMESPACE: &str = "stop-slop-stand";
/// The default limit of a call into a guest or the SDK.
const GUEST_CALL: Duration = Duration::from_secs(30);
/// `create` includes a cold image pull.
const CREATE: Duration = Duration::from_secs(600);
const POLL: Duration = Duration::from_millis(25);
const METRICS_EVERY: Duration = Duration::from_secs(1);

pub struct Microsandbox {
    runtime: tokio::runtime::Runtime,
    inner: Arc<Inner>,
}

struct Inner {
    home: Utf8PathBuf,
    local: OnceCell<Arc<msb::LocalBackend>>,
    installed: OnceCell<guest::Binary>,
    live: Mutex<BTreeMap<String, Arc<Live>>>,
    ports: Mutex<network::Ports>,
    seq: AtomicU64,
}

/// A VM this process created and has not destroyed. Dropping `vm` would stop it (the SDK's safety
/// net for an attached VM), so it lives here until `destroy`.
struct Live {
    name: String,
    vm: msb::Sandbox,
    launch: Launch,
    image: process::ImageDefaults,
    folder: Utf8PathBuf,
    /// Guest paths of the disks the VM owns: its volumes and the version disk.
    owned: Vec<String>,
    service: Job,
    execs: Mutex<BTreeMap<String, Job>>,
    peak: Arc<AtomicU64>,
    sampler: tokio::task::AbortHandle,
}

impl Microsandbox {
    /// Cheap: the runtime of microsandbox and `stand-guest` are installed on the first `pull` or
    /// `create`, which the stand always calls under its lock.
    pub fn new(home: &Utf8Path) -> Result<Microsandbox, SandboxError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(16 << 20)
            .build()
            .map_err(|error| backend(format!("start the async runtime: {error}")))?;
        Ok(Microsandbox {
            runtime,
            inner: Arc::new(Inner {
                home: home.to_owned(),
                local: OnceCell::new(),
                installed: OnceCell::new(),
                live: Mutex::new(BTreeMap::new()),
                ports: Mutex::new(network::Ports::default()),
                seq: AtomicU64::new(0),
            }),
        })
    }

    fn block<T, F>(&self, work: impl FnOnce(Arc<Inner>) -> F) -> Result<T, SandboxError>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, SandboxError>> + Send + 'static,
    {
        let task = work(Arc::clone(&self.inner));
        self.runtime.block_on(async {
            tokio::spawn(task)
                .await
                .unwrap_or_else(|error| Err(backend(format!("adapter task: {error}"))))
        })
    }
}

impl Sandbox for Microsandbox {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn pull(&self, images: &[ImageRef]) -> Result<Pulled, SandboxError> {
        let images = images.to_vec();
        self.block(|inner| async move { inner.pull(images).await })
    }

    fn create(&self, batch: &[Launch]) -> Result<Vec<SandboxId>, SandboxError> {
        let batch = batch.to_vec();
        self.block(|inner| async move { inner.create(batch).await })
    }

    fn exec_detached(&self, id: &SandboxId, argv: &[String]) -> Result<ExecId, SandboxError> {
        let (id, argv) = (id.clone(), argv.to_vec());
        self.block(|inner| async move {
            let live = inner.live(&id)?;
            let job = process::exec_detached(&live, &argv).await?;
            let exec = format!("{}/{}", live.name, job.id());
            live.execs.lock().unwrap().insert(exec.clone(), job);
            Ok(ExecId::new(exec))
        })
    }

    fn wait(&self, what: &[Waitable], deadline: Instant) -> Result<Option<Exited>, SandboxError> {
        let what = what.to_vec();
        self.block(|inner| async move { inner.wait(what, deadline).await })
    }

    fn exec(
        &self,
        id: &SandboxId,
        argv: &[String],
        deadline: Instant,
        stdout: &Utf8Path,
    ) -> Result<ExecOutcome, SandboxError> {
        let (id, argv, stdout) = (id.clone(), argv.to_vec(), stdout.to_owned());
        self.block(|inner| async move {
            process::exec(&*inner.live(&id)?, &argv, deadline, &stdout).await
        })
    }

    fn stop(&self, id: &SandboxId) -> Result<Stopped, SandboxError> {
        let id = id.clone();
        self.block(|inner| async move { process::stop(&*inner.live(&id)?).await })
    }

    fn fault(&self, ids: &[SandboxId], fault: &Fault) -> Result<(), SandboxError> {
        let (ids, fault) = (ids.to_vec(), fault.clone());
        self.block(|inner| async move {
            let lives: Vec<_> = ids
                .iter()
                .map(|id| inner.live(id))
                .collect::<Result<_, _>>()?;
            network::fault(&lives, &fault).await
        })
    }

    fn metrics(&self, id: &SandboxId) -> Result<Metrics, SandboxError> {
        let id = id.clone();
        self.block(|inner| async move { inner.live(&id)?.metrics().await })
    }

    fn logs(&self, id: &SandboxId) -> Result<Vec<u8>, SandboxError> {
        let id = id.clone();
        self.block(|inner| async move {
            let live = inner.live(&id)?;
            let options = JobLogOptions::default();
            let entries = within(GUEST_CALL, "read the logs", live.service.logs(&options))
                .await?
                .map_err(backend)?;
            Ok(entries.into_iter().flat_map(|entry| entry.data).collect())
        })
    }

    fn copy_out(&self, id: &SandboxId, guest: &str, host: &Utf8Path) -> Result<(), SandboxError> {
        let (id, guest, host) = (id.clone(), guest.to_owned(), host.to_owned());
        self.block(
            |inner| async move { process::copy_out(&*inner.live(&id)?, &guest, &host).await },
        )
    }

    fn volume_sizes(&self, id: &SandboxId) -> Result<Vec<(VolumeName, u64)>, SandboxError> {
        let id = id.clone();
        self.block(|inner| async move { volumes::sizes(&*inner.live(&id)?).await })
    }

    fn snapshot(&self, id: &SandboxId) -> Result<SnapshotId, SandboxError> {
        let id = id.clone();
        self.block(|inner| async move {
            let live = inner.live(&id)?;
            let replica = format!("{}-{}", live.launch.service, live.launch.replica);
            volumes::snapshot(&live, &inner.name("snap", &replica)).await
        })
    }

    fn destroy(&self, id: &SandboxId) -> Result<(), SandboxError> {
        let id = id.clone();
        self.block(|inner| async move { inner.destroy(&id).await })
    }

    fn drop_snapshot(&self, snapshot: &SnapshotId) -> Result<(), SandboxError> {
        let snapshot = snapshot.clone();
        self.block(|inner| async move {
            inner.local().await?;
            let Some(found) = volumes::listed(&snapshot).await? else {
                return Ok(());
            };
            let remove = msb::Snapshot::remove_ref(found.reference(), true);
            match within(GUEST_CALL, "drop a snapshot", remove).await? {
                Ok(()) | Err(msb::MicrosandboxError::SnapshotNotFound(_)) => Ok(()),
                Err(error) => Err(backend(error)),
            }
        })
    }

    fn leftovers(&self) -> Result<Leftovers, SandboxError> {
        self.block(|inner| async move { inner.leftovers().await })
    }

    fn prune_images(&self, keep: &[ImageRef]) -> Result<(), SandboxError> {
        let keep = keep.to_vec();
        self.block(|inner| async move { inner.prune_images(keep).await })
    }
}

impl Inner {
    /// The process-wide SDK backend on `home`. Calls of the SDK that spawn their own tasks resolve
    /// the default backend, not a scoped one, so there is one home per process.
    async fn local(&self) -> Result<&Arc<msb::LocalBackend>, SandboxError> {
        self.local
            .get_or_try_init(|| async {
                static HOME: OnceLock<Utf8PathBuf> = OnceLock::new();
                let first = HOME.get_or_init(|| self.home.clone());
                if *first != self.home {
                    return Err(backend(format!(
                        "one microsandbox home per process, and {first} is already in use"
                    )));
                }
                // The home's own config file keeps the user's `~/.microsandbox/config.json` out.
                let local = msb::LocalBackend::builder()
                    .home(self.home.as_std_path())
                    .config_path(self.home.join("config.json").as_std_path())
                    .build_lazy()
                    .map_err(backend)?;
                let local = Arc::new(local);
                let shared: Arc<dyn msb::Backend> = local.clone();
                msb::set_default_backend(shared);
                Ok(local)
            })
            .await
    }

    async fn install(&self) -> Result<&guest::Binary, SandboxError> {
        self.installed
            .get_or_try_init(|| async {
                let local = self.local().await?;
                let options = msb::setup::InstallOptions::default();
                let runtime = msb::setup::ensure_runtime(local.config(), options);
                within(CREATE, "install the microsandbox runtime", runtime)
                    .await?
                    .map_err(backend)?;
                guest::install(&self.home, guest::local_build()).await
            })
            .await
    }

    fn live(&self, id: &SandboxId) -> Result<Arc<Live>, SandboxError> {
        self.live
            .lock()
            .unwrap()
            .get(id.as_str())
            .cloned()
            .ok_or_else(|| SandboxError::Gone(id.clone()))
    }

    /// Unique across processes: a crashed run's VMs may still be there when the next one starts.
    fn name(&self, kind: &str, service: &str) -> String {
        let since = SystemTime::now().duration_since(UNIX_EPOCH);
        let millis = since.unwrap_or_default().as_millis();
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let mut name = format!("{NAMESPACE}-{millis:x}-{seq}-{kind}-{service}");
        name.truncate(msb::MAX_SANDBOX_NAME_BYTES);
        name
    }

    /// The adapter's folder of one VM, removed with it.
    fn folder(&self, name: &str) -> Utf8PathBuf {
        self.home.join("vm").join(name)
    }

    async fn pull(&self, images: Vec<ImageRef>) -> Result<Pulled, SandboxError> {
        let binary = self.install().await?;
        let mut digests = Vec::with_capacity(images.len());
        for image in images {
            let handle = match msb::Image::get(image.as_str()).await {
                Ok(handle) => handle,
                Err(msb::MicrosandboxError::ImageNotFound(_)) => {
                    self.pull_through_a_vm(&image).await?;
                    msb::Image::get(image.as_str()).await.map_err(backend)?
                }
                Err(error) => return Err(backend(error)),
            };
            let digest = handle.manifest_digest().ok_or_else(|| {
                backend(format!(
                    "microsandbox gave no digest for `{image}` after the pull"
                ))
            })?;
            digests.push((image.clone(), digest.to_owned()));
        }
        Ok(Pulled {
            digests,
            guest: binary.version.clone(),
        })
    }

    /// SDK 0.7.8 pulls only as part of `create`, so a missing image costs one VM boot. A cached
    /// image is never fetched again: what `create` runs is what `pull` reported.
    async fn pull_through_a_vm(&self, image: &ImageRef) -> Result<(), SandboxError> {
        let name = self.name("pull", "image");
        // Held until the destroy: dropping an attached VM stops it on its own.
        let _vm = volumes::create_with_retry(&name, || {
            msb::Sandbox::builder(&name)
                .image(image.as_str())
                .cpus(1)
                .memory(volumes::PULL_MEMORY_MIB)
                .label(NAMESPACE, "1")
        })
        .await
        .map_err(|error| image_error(image, error))?;
        destroy_vm(&name).await
    }

    async fn create(self: &Arc<Self>, batch: Vec<Launch>) -> Result<Vec<SandboxId>, SandboxError> {
        let wired = {
            let mut ports = self.ports.lock().unwrap();
            let wire = |launch: &Launch| network::wire(&mut ports, launch);
            batch.iter().map(wire).collect::<Result<Vec<_>, _>>()?
        };
        let binary = self.install().await?.clone();
        let mut tasks = tokio::task::JoinSet::new();
        for (index, (launch, wiring)) in batch.into_iter().zip(wired).enumerate() {
            let name = self.name("vm", &format!("{}-{}", launch.service, launch.replica));
            let (inner, binary) = (Arc::clone(self), binary.clone());
            tasks.spawn(async move { (index, inner.start(name, launch, wiring, &binary).await) });
        }
        let mut started = Vec::new();
        let mut failure = None;
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((index, Ok(live))) => started.push((index, live)),
                Ok((_, Err(error))) => {
                    failure.get_or_insert(error);
                }
                Err(error) => {
                    failure.get_or_insert(backend(format!("start a VM: {error}")));
                }
            }
        }
        if let Some(error) = failure {
            for (_, live) in started {
                let _ = self.remove(&live).await;
            }
            return Err(error);
        }
        started.sort_by_key(|(index, _)| *index);
        let mut map = self.live.lock().unwrap();
        Ok(started
            .into_iter()
            .map(|(_, live)| {
                let id = SandboxId::new(live.name.clone());
                map.insert(live.name.clone(), Arc::new(live));
                id
            })
            .collect())
    }

    /// The VM, its scratch folders and version tree, its network, then the service under
    /// `stand-guest run`. Returns once the service's command runs.
    async fn start(
        &self,
        name: String,
        launch: Launch,
        wiring: network::Wiring,
        binary: &guest::Binary,
    ) -> Result<Live, SandboxError> {
        let folder = self.folder(&name);
        volumes::make_folder(&process::run_dir(&folder))?;
        let net = network::net_dir(&folder);
        std::fs::create_dir_all(&net).map_err(|error| backend(format!("{net}: {error}")))?;
        for mount in &launch.mounts {
            if let Mount::Bind {
                host,
                read_only: false,
                ..
            } = mount
            {
                volumes::open_to_every_user(host)?;
            }
        }
        let made = match &launch.seed {
            Some(Seed::Snapshot(snapshot)) => {
                let published = &wiring.published;
                volumes::restore(&name, &launch, snapshot, published, &folder, binary).await
            }
            _ => volumes::fresh(&name, &launch, &wiring.published, &folder, binary).await,
        };
        let (vm, owned) = match made {
            Ok(made) => made,
            Err(error) => {
                remove_folder(&folder);
                return Err(error);
            }
        };
        // The image's config is read once the VM is made, since its create pulls a missing image.
        let started = async {
            let image = process::image_defaults(&launch).await?;
            lay_out(&vm, &launch).await?;
            network::start(&vm, &wiring, &folder).await?;
            let service = process::start_service(&vm, &launch, &image, &folder).await?;
            Ok((image, service))
        };
        match started.await {
            Ok((image, service)) => {
                let peak = Arc::new(AtomicU64::new(0));
                let sampler = sample(vm.clone(), Arc::clone(&peak));
                Ok(Live {
                    name,
                    vm,
                    launch,
                    image,
                    folder,
                    owned,
                    service,
                    execs: Mutex::new(BTreeMap::new()),
                    peak,
                    sampler,
                })
            }
            Err(error) => {
                let _ = destroy_vm(&name).await;
                remove_folder(&folder);
                Err(error)
            }
        }
    }

    async fn wait(
        &self,
        what: Vec<Waitable>,
        deadline: Instant,
    ) -> Result<Option<Exited>, SandboxError> {
        loop {
            for waitable in &what {
                if let Some(code) = self.exit_of(waitable).await? {
                    return Ok(Some(Exited {
                        what: waitable.clone(),
                        code,
                        at: Instant::now(),
                    }));
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            tokio::time::sleep(POLL.min(deadline - now)).await;
        }
    }

    /// `None` while it runs, else its code; a code of `None` when it was killed without one.
    async fn exit_of(&self, what: &Waitable) -> Result<Option<Option<i32>>, SandboxError> {
        match what {
            Waitable::Sandbox(id) => {
                let live = self.live(id)?;
                let info = inspect(&live.service, id).await?;
                if info.state.is_active() {
                    return Ok(None);
                }
                process::service_code(&live, &info).await.map(Some)
            }
            Waitable::Exec(exec) => {
                let vm = exec.as_str().split_once('/').map_or("", |(vm, _)| vm);
                let id = SandboxId::new(vm.to_owned());
                let job = self
                    .live(&id)?
                    .execs
                    .lock()
                    .unwrap()
                    .get(exec.as_str())
                    .cloned();
                let job = job.ok_or_else(|| backend(format!("no exec `{exec}`")))?;
                let info = inspect(&job, &id).await?;
                Ok(match info.state {
                    state if state.is_active() => None,
                    JobState::Exited => Some(info.exit_code),
                    _ => Some(None),
                })
            }
        }
    }

    async fn destroy(&self, id: &SandboxId) -> Result<(), SandboxError> {
        let live = self.live.lock().unwrap().remove(id.as_str());
        match live {
            Some(live) => self.remove(&live).await,
            // A VM of an earlier process, which `leftovers` found.
            None => {
                self.local().await?;
                destroy_vm(id.as_str()).await?;
                remove_folder(&self.folder(id.as_str()));
                Ok(())
            }
        }
    }

    async fn remove(&self, live: &Live) -> Result<(), SandboxError> {
        live.sampler.abort();
        destroy_vm(&live.name).await?;
        remove_folder(&live.folder);
        Ok(())
    }

    async fn leftovers(&self) -> Result<Leftovers, SandboxError> {
        let local = self.local().await?;
        if !msb::setup::is_runtime_installed(local.config()) {
            return Ok(Leftovers::default());
        }
        let mut sandboxes = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let after = cursor.take();
            let page = msb::Sandbox::list_with(|list| {
                let list = list.limit(msb::sandbox::MAX_SANDBOX_LIST_LIMIT);
                match after {
                    Some(after) => list.cursor(after),
                    None => list,
                }
            })
            .await
            .map_err(backend)?;
            let names = page.sandboxes.iter().map(|handle| handle.name());
            sandboxes.extend(
                names
                    .filter(|name| ours(name))
                    .map(|name| SandboxId::new(name.to_owned())),
            );
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        let snapshots = msb::Snapshot::list()
            .await
            .map_err(backend)?
            .into_iter()
            .filter(|snapshot| snapshot.name().is_some_and(ours))
            .map(|snapshot| SnapshotId::new(snapshot.digest().to_owned()))
            .collect();
        Ok(Leftovers {
            sandboxes,
            snapshots,
        })
    }

    async fn prune_images(&self, keep: Vec<ImageRef>) -> Result<(), SandboxError> {
        let local = self.local().await?;
        if !msb::setup::is_runtime_installed(local.config()) {
            return Ok(());
        }
        for image in msb::Image::list().await.map_err(backend)? {
            if keep.iter().any(|kept| kept.as_str() == image.reference()) {
                continue;
            }
            // A snapshot pins its image: without force the SDK refuses, and the image stays. A
            // removal also drops the manifest and layers no other reference holds; the SDK's
            // `Image::prune` would drop every image no VM or snapshot uses, kept ones too.
            match msb::Image::remove(image.reference(), false).await {
                Ok(())
                | Err(msb::MicrosandboxError::ImageInUse(_))
                | Err(msb::MicrosandboxError::ImageNotFound(_)) => {}
                Err(error) => return Err(backend(error)),
            }
        }
        Ok(())
    }
}

impl Live {
    async fn metrics(&self) -> Result<Metrics, SandboxError> {
        let now = within(GUEST_CALL, "read the metrics", self.vm.metrics())
            .await?
            .map_err(backend)?;
        let peak = self.peak.fetch_max(now.memory_bytes, Ordering::Relaxed);
        let (ports, unexposed) = guest::traffic(&network::counters(&self.folder))?;
        Ok(Metrics {
            cpu: Duration::from_nanos(now.vcpu_time_ns),
            memory_peak: peak.max(now.memory_bytes),
            memory_limit: now.memory_limit_bytes,
            ports,
            unexposed,
        })
    }
}

/// The scratch folders and the version tree, before the service starts.
async fn lay_out(vm: &msb::Sandbox, launch: &Launch) -> Result<(), SandboxError> {
    let scratch: Vec<&str> = launch
        .mounts
        .iter()
        .filter_map(|mount| match mount {
            Mount::Scratch { guest } => Some(guest.as_str()),
            _ => None,
        })
        .collect();
    let tree = match &launch.seed {
        Some(Seed::Tree { tar, at }) => Some((tar, at)),
        _ => None,
    };
    if scratch.is_empty() && tree.is_none() {
        return Ok(());
    }
    let owner = process::owner(vm).await?;
    process::make_scratch(vm, &scratch, owner).await?;
    if let Some((tar, at)) = tree {
        volumes::seed_tree(vm, tar, at, owner).await?;
    }
    Ok(())
}

fn sample(vm: msb::Sandbox, peak: Arc<AtomicU64>) -> tokio::task::AbortHandle {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(METRICS_EVERY);
        loop {
            tick.tick().await;
            if let Ok(now) = vm.metrics().await {
                peak.fetch_max(now.memory_bytes, Ordering::Relaxed);
            }
        }
    })
    .abort_handle()
}

/// A job of a VM that was removed is `Gone`.
async fn inspect(job: &Job, id: &SandboxId) -> Result<msb::jobs::JobInfo, SandboxError> {
    match within(GUEST_CALL, "inspect a job", job.inspect()).await? {
        Ok(info) => Ok(info),
        Err(msb::JobError::Sandbox(msb::MicrosandboxError::SandboxNotFound(_))) => {
            Err(SandboxError::Gone(id.clone()))
        }
        Err(error) => Err(backend(error)),
    }
}

/// Forced: the service has stopped or no longer matters. A destroy that hangs gets a kill, then
/// one more destroy.
async fn destroy_vm(name: &str) -> Result<(), SandboxError> {
    let handle = match within(GUEST_CALL, "find a VM", msb::Sandbox::get(name)).await? {
        Ok(handle) => handle,
        Err(msb::MicrosandboxError::SandboxNotFound(_)) => return Ok(()),
        Err(error) => return Err(backend(error)),
    };
    let options = DestroyOptions {
        force: true,
        timeout: GUEST_CALL,
    };
    if let Ok(result) = timeout(GUEST_CALL, handle.destroy_with(options)).await
        && removed(result).is_ok()
    {
        return Ok(());
    }
    let _ = timeout(GUEST_CALL, handle.kill()).await;
    let again = handle.destroy_with(options);
    removed(within(GUEST_CALL, "destroy a VM after a kill", again).await?)
}

fn removed(result: Result<(), msb::MicrosandboxError>) -> Result<(), SandboxError> {
    match result {
        Ok(()) | Err(msb::MicrosandboxError::SandboxNotFound(_)) => Ok(()),
        Err(error) => Err(backend(error)),
    }
}

fn ours(name: &str) -> bool {
    name.strip_prefix(NAMESPACE)
        .is_some_and(|rest| rest.starts_with('-'))
}

fn remove_folder(folder: &Utf8Path) {
    let _ = std::fs::remove_dir_all(folder);
}

/// A call into the guest or the SDK past `limit` is a stand failure, not a hang.
async fn within<T>(
    limit: Duration,
    what: &str,
    call: impl Future<Output = T>,
) -> Result<T, SandboxError> {
    timeout(limit, call).await.map_err(|_| SandboxError::Guest {
        message: format!("{what}: no answer within {} s", limit.as_secs()),
    })
}

fn backend(error: impl Display) -> SandboxError {
    SandboxError::Backend {
        message: error.to_string(),
    }
}

/// The SDK does not export the registry's error type, so a missing image is told by its text;
/// Docker Hub answers a repository that does not exist with 401, so "not authorized" counts too.
fn image_error(image: &ImageRef, error: msb::MicrosandboxError) -> SandboxError {
    const NOT_FOUND: [&str; 5] = [
        "manifest unknown",
        "name unknown",
        "image manifest not found",
        "not authorized",
        "no manifest for platform",
    ];
    let text = error.to_string();
    let lower = text.to_lowercase();
    if matches!(error, msb::MicrosandboxError::ImageNotFound(_))
        || NOT_FOUND.iter().any(|hint| lower.contains(hint))
    {
        SandboxError::ImageNotFound(image.clone())
    } else if matches!(error, msb::MicrosandboxError::Image(_)) {
        SandboxError::Registry { message: text }
    } else {
        backend(text)
    }
}
