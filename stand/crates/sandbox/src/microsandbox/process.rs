//! The service under `stand-guest run`, its `stop`, probes under `stand-guest exec`, healthcheck
//! attempts, `Scratch` folders and `copy_out`.
//!
//! A VM gets no env of its own, fresh or restored (a snapshot carries none, plan row 87): the env,
//! the workdir and the command of the `Launch` go into the exec of the service, and the same env
//! and workdir into every later exec of the VM.

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ::guest::{Command, ExecArgs, ExecEnd, ExecStatus, RunArgs, RunStatus, StopArgs, StopReport};
use ::microsandbox as msb;
use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use microsandbox_image::{GlobalCache, Reference};
use msb::jobs::{Job, JobInfo, JobLogOptions, JobState};
use msb::sandbox::{ExecOptionsBuilder, FsSetAttrs};

use super::{GUEST_CALL, Live, POLL, backend, exec_error, guest, guest_error, network, within};
use crate::{ExecOutcome, ImageRef, Launch, Mount, SandboxError, Stopped};

/// Where `run` keeps its status file and the service's output: the service's user writes them, so
/// it is a bind of its own and `/stand/net`, which root writes, stays out of that user's reach.
pub(super) const RUN_DIR: &str = "/stand/run";
const STATUS: &str = "status.json";
const LOG: &str = "output.log";
/// Room for `stand-guest` to act at its own deadline and report before the SDK gives up.
const MARGIN: Duration = Duration::from_secs(10);
/// In `/stand/net`, where only root writes: a link the service's user put at a name in a folder of
/// its own could turn root's write anywhere. The tar reaches the host through the bind.
const COPY_TAR: &str = "copy.tar";
const STOP_SIGNAL: &str = "SIGTERM";
const COPY: Duration = Duration::from_secs(300);
/// `run` writes its command's last output into the log for at most 1 s after the end.
const RUN_END: Duration = Duration::from_secs(5);

/// The host side of `RUN_DIR` in the adapter's folder of a VM.
pub(super) fn run_dir(folder: &Utf8Path) -> Utf8PathBuf {
    folder.join("run")
}

fn status_in_guest() -> PathBuf {
    PathBuf::from(format!("{RUN_DIR}/{STATUS}"))
}

/// From the image's config; a `Launch` field, when set, wins.
#[derive(Debug, Clone)]
pub(super) struct ImageDefaults {
    /// `USER` as the image writes it; empty is root.
    pub user: String,
    pub workdir: Option<String>,
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
    /// What `stop` sends: the `Launch`'s, else the image's `STOPSIGNAL`, else `SIGTERM`, as Docker.
    pub stop_signal: String,
}

pub(super) async fn image_defaults(
    local: &msb::LocalBackend,
    launch: &Launch,
) -> Result<ImageDefaults, SandboxError> {
    let detail = msb::Image::inspect(launch.image.as_str())
        .await
        .map_err(backend)?;
    let stop_signal = match &launch.stop_signal {
        Some(signal) => signal.clone(),
        None => image_stop_signal(local, &launch.image)
            .await?
            .unwrap_or_else(|| STOP_SIGNAL.to_owned()),
    };
    // At the start rather than at the stop, after the work of a whole repeat.
    if ::guest::signal_number(&stop_signal).is_none() {
        return Err(backend(format!(
            "`{}` would stop with `{stop_signal}`, which stand-guest does not send; set its \
             `stop_signal`",
            launch.service
        )));
    }
    let config = detail.config;
    Ok(ImageDefaults {
        stop_signal,
        user: config
            .as_ref()
            .and_then(|c| c.user.clone())
            .unwrap_or_default(),
        workdir: config
            .as_ref()
            .and_then(|c| c.working_dir.clone())
            .filter(|dir| !dir.is_empty()),
        entrypoint: config
            .as_ref()
            .and_then(|c| c.entrypoint.clone())
            .unwrap_or_default(),
        cmd: config.and_then(|c| c.cmd).unwrap_or_default(),
    })
}

/// SDK 0.7.8 never stores `STOPSIGNAL` in its image database, so `Image::inspect` gives `None` for
/// every image; the image's metadata in the SDK's cache has it, and the SDK's own reader finds it.
async fn image_stop_signal(
    local: &msb::LocalBackend,
    image: &ImageRef,
) -> Result<Option<String>, SandboxError> {
    let reference: Reference = image
        .as_str()
        .parse()
        .map_err(|error| backend(format!("`{image}`: {error}")))?;
    let cache = GlobalCache::new(&local.cache_dir()).map_err(backend)?;
    let metadata = cache.read_image_metadata_async(&reference).await;
    let metadata = metadata
        .map_err(backend)?
        .ok_or_else(|| backend(format!("no metadata of `{image}` in the image cache")))?;
    Ok(metadata
        .config
        .stop_signal
        .filter(|signal| !signal.is_empty())
        .map(|signal| docker_signal_name(&signal)))
}

/// Docker reads `STOPSIGNAL` in any case and with or without `SIG`, `quit` as `SIGQUIT`, the one
/// form `stand-guest` takes. A number stays as written, and `image_defaults` refuses it.
fn docker_signal_name(signal: &str) -> String {
    let upper = signal.trim().to_ascii_uppercase();
    if upper.starts_with("SIG") || upper.bytes().all(|byte| byte.is_ascii_digit()) {
        upper
    } else {
        format!("SIG{upper}")
    }
}

/// As Compose combines them: a new `entrypoint` drops the image's CMD unless `command` is set too.
pub(super) fn command(launch: &Launch, image: &ImageDefaults) -> Result<Vec<String>, SandboxError> {
    let (entrypoint, cmd) = match (&launch.entrypoint, &launch.command) {
        (Some(entrypoint), Some(command)) => (entrypoint.clone(), command.clone()),
        (Some(entrypoint), None) => (entrypoint.clone(), Vec::new()),
        (None, Some(command)) => (image.entrypoint.clone(), command.clone()),
        (None, None) => (image.entrypoint.clone(), image.cmd.clone()),
    };
    let argv = [entrypoint, cmd].concat();
    if argv.is_empty() {
        return Err(backend(format!(
            "`{}` has no ENTRYPOINT or CMD and the service sets no command",
            launch.image
        )));
    }
    Ok(argv)
}

fn with_env(
    options: ExecOptionsBuilder,
    launch: &Launch,
    image: &ImageDefaults,
) -> ExecOptionsBuilder {
    let options = options.envs(
        launch
            .env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    match launch.workdir.as_ref().or(image.workdir.as_ref()) {
        Some(dir) => options.cwd(dir.clone()),
        None => options,
    }
}

/// As the image's user (the SDK's default for an exec), so the service runs as it would in
/// Compose. `run` writes its status file once the command is spawned: that is `service_started`.
pub(super) async fn start_service(
    vm: &msb::Sandbox,
    launch: &Launch,
    image: &ImageDefaults,
    folder: &Utf8Path,
) -> Result<Job, SandboxError> {
    let run = RunArgs {
        status: status_in_guest(),
        log: PathBuf::from(format!("{RUN_DIR}/{LOG}")),
        command: command(launch, image)?,
    };
    let args = Command::Run(run).to_args();
    let start = vm.exec_detached_with(guest::BIN, |options| {
        with_env(options.args(args).stdin_null(), launch, image)
    });
    let job = within(GUEST_CALL, "start the service", start)
        .await?
        .map_err(backend)?;
    match wait_for_file(&job, &run_dir(folder).join(STATUS)).await? {
        None => Ok(job),
        // The command never started (not found, not executable): the app's failure, whose code
        // `wait` gives and whose reason is in the log.
        Some(info) if never_started(&info) => Ok(job),
        Some(info) => Err(ended(&job, "stand-guest run", &info).await),
    }
}

fn never_started(info: &JobInfo) -> bool {
    info.state == JobState::Exited && matches!(info.exit_code, Some(126 | 127))
}

/// The code of a service whose `run` ended: from the status file, or `run`'s 126 or 127 when the
/// command never started; `None` when the VM went away under it. Anything else is `run` failing
/// itself, 125 among it.
pub(super) async fn service_code(live: &Live, info: &JobInfo) -> Result<Option<i32>, SandboxError> {
    let path = run_dir(&live.folder).join(STATUS);
    let recorded = match std::fs::read_to_string(&path) {
        Ok(text) => {
            RunStatus::parse(&text)
                .map_err(|error| guest_error(format!("{path}: {error}")))?
                .code
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(backend(format!("{path}: {error}"))),
    };
    match recorded {
        Some(code) => Ok(Some(code)),
        None if never_started(info) => Ok(info.exit_code),
        None if info.state == JobState::Lost => Ok(None),
        None => Err(ended(&live.service, "stand-guest run", info).await),
    }
}

/// Healthcheck attempts, as the image's user with the VM's env. At `timeout` the SDK's runtime marks
/// the job timed out and has the agent kill its process group. `None`: the program did not start,
/// which Docker counts as a failed attempt, not as an error.
pub(super) async fn exec_detached(
    live: &Live,
    argv: &[String],
    timeout: Duration,
) -> Result<Option<Job>, SandboxError> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| backend("an empty healthcheck command"))?;
    let args = args.to_vec();
    // The runtime refuses a timeout of 0; an attempt with no time left dies at once.
    let timeout = timeout.max(Duration::from_millis(1));
    let start = live.vm.exec_detached_with(program.clone(), |options| {
        let options = options.args(args).stdin_null().timeout(timeout);
        with_env(options, &live.launch, &live.image)
    });
    match within(GUEST_CALL, "start a healthcheck", start).await? {
        Ok(job) => Ok(Some(job)),
        Err(msb::JobError::Operation { code, .. }) if code == "exec_failed" => Ok(None),
        Err(error) => Err(backend(error)),
    }
}

pub(super) async fn stop(live: &Live) -> Result<Stopped, SandboxError> {
    // `create` returns once `run` wrote the file, so without it the command never started, and
    // `stand-guest stop` would fail on the missing file.
    if !run_dir(&live.folder).join(STATUS).exists() {
        return Ok(Stopped::default());
    }
    let grace = live.launch.stop_grace;
    let args = Command::Stop(StopArgs {
        status: status_in_guest(),
        signal: live.image.stop_signal.clone(),
        grace,
    })
    .to_args();
    let output = root(&live.vm, guest::BIN, args, grace + MARGIN).await?;
    if output.status().code != 0 {
        return Err(guest_failed("stand-guest stop", &output));
    }
    let report = StopReport::parse(&String::from_utf8_lossy(output.stdout_bytes()))
        .map_err(|error| guest_error(error.to_string()))?;
    // `run` writes the command's last output into the log after it records the code, so the stop
    // returns once the job has ended too, and `logs` has it all. Without a code `run` still waits
    // for the command, and the stop does not.
    if report.code.is_some() {
        let ended = async {
            loop {
                match live.service.inspect().await {
                    Ok(info) if info.state.is_active() => tokio::time::sleep(POLL).await,
                    _ => break,
                }
            }
        };
        let _ = tokio::time::timeout(RUN_END, ended).await;
    }
    Ok(Stopped {
        after: Duration::from_millis(report.after_ms),
        killed: report.killed,
    })
}

/// A probe or `inspect`: root starts `stand-guest exec`, which drops to the probe group and the
/// image's user, with the VM's env and workdir. Its outputs reach the host through the VM's bind
/// that holds `stdout`; `<stdout>.status` says how the command ended and goes once read.
pub(super) async fn exec(
    live: &Live,
    argv: &[String],
    deadline: Instant,
    stdout: &Utf8Path,
) -> Result<ExecOutcome, SandboxError> {
    let left = deadline.saturating_duration_since(Instant::now());
    let args = Command::Exec(ExecArgs {
        out: PathBuf::from(guest_path(&live.launch.mounts, stdout)?),
        deadline: left,
        user: live.image.user.clone(),
        argv: argv.to_vec(),
    })
    .to_args();
    let output = run_for(&live.vm, guest::BIN, left + MARGIN, |options| {
        with_env(options.args(args).user("root"), &live.launch, &live.image)
    })
    .await?;
    if output.status().code != 0 {
        return Err(guest_failed("stand-guest exec", &output));
    }
    let path = ExecStatus::path(stdout.as_std_path());
    let read = std::fs::read_to_string(&path);
    let _ = std::fs::remove_file(&path);
    let shown = path.display();
    let text = read.map_err(|error| guest_error(format!("{shown}: {error}")))?;
    let status =
        ExecStatus::parse(&text).map_err(|error| guest_error(format!("{shown}: {error}")))?;
    Ok(match (status.timed_out, status.end) {
        (true, _) => ExecOutcome::TimedOut,
        (false, ExecEnd::Code(code)) => ExecOutcome::Exited(code),
        (false, ExecEnd::Signal(signal)) => ExecOutcome::Exited(128 + signal),
    })
}

/// `stdout` in the guest: under the guest path of the writable bind whose host folder holds it,
/// the deepest one when they nest.
fn guest_path(mounts: &[Mount], stdout: &Utf8Path) -> Result<String, SandboxError> {
    let under = mounts.iter().filter_map(|mount| match mount {
        Mount::Bind {
            host,
            guest,
            read_only: false,
        } => {
            let rest = stdout.strip_prefix(host).ok()?;
            let parts: Option<Vec<&str>> = rest
                .components()
                .map(|part| match part {
                    Utf8Component::Normal(name) => Some(name),
                    _ => None,
                })
                .collect();
            let parts = parts.filter(|parts| !parts.is_empty())?;
            Some((host.as_str().len(), guest, parts))
        }
        _ => None,
    });
    let (_, guest, parts) = under.max_by_key(|(depth, ..)| *depth).ok_or_else(|| {
        guest_error(format!(
            "{stdout} is not a file under a writable bind of the VM"
        ))
    })?;
    Ok(format!(
        "{}/{}",
        guest.trim_end_matches('/'),
        parts.join("/")
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Owner {
    uid: u32,
    gid: u32,
}

/// Whom the agent runs an exec as by default, the image's user: `id` resolves it in the guest as
/// the service's processes see it, whatever form `USER` has in the image.
pub(super) async fn owner(vm: &msb::Sandbox) -> Result<Owner, SandboxError> {
    let output = run_for(vm, "id", GUEST_CALL, |options| options).await?;
    let text = String::from_utf8_lossy(output.stdout_bytes());
    match parse_id(&text) {
        Some(owner) if output.status().code == 0 => Ok(owner),
        _ => Err(guest_failed("id", &output)),
    }
}

/// `uid=101(nginx) gid=101(nginx) groups=...`, as GNU and busybox `id` print it.
fn parse_id(text: &str) -> Option<Owner> {
    let number = |key: &str| {
        let value = text
            .split_whitespace()
            .find_map(|word| word.strip_prefix(key))?;
        let digits = value.split(|c: char| !c.is_ascii_digit()).next()?;
        digits.parse().ok()
    };
    Some(Owner {
        uid: number("uid=")?,
        gid: number("gid=")?,
    })
}

/// Through the agent, which is root.
pub(super) async fn give(
    vm: &msb::Sandbox,
    path: &str,
    Owner { uid, gid }: Owner,
) -> Result<(), SandboxError> {
    let attrs = FsSetAttrs {
        uid: Some(uid),
        gid: Some(gid),
        ..FsSetAttrs::default()
    };
    let fs = vm.fs();
    within(GUEST_CALL, "give a folder", fs.set_stat(path, false, attrs))
        .await?
        .map_err(|error| backend(format!("give {path} to {uid}:{gid}: {error}")))
}

/// Empty and the image user's in every VM: the root disk of a VM from a snapshot may still hold
/// the folder of the VM it was taken from.
pub(super) async fn make_scratch(
    vm: &msb::Sandbox,
    folders: &[&str],
    owner: Owner,
) -> Result<(), SandboxError> {
    let fs = vm.fs();
    for &folder in folders {
        let found = within(GUEST_CALL, "look for a scratch folder", fs.exists(folder)).await?;
        if found.map_err(backend)? {
            within(GUEST_CALL, "empty a scratch folder", fs.remove_dir(folder))
                .await?
                .map_err(backend)?;
        }
        within(GUEST_CALL, "make a scratch folder", fs.mkdir(folder))
            .await?
            .map_err(backend)?;
        give(vm, folder, owner).await?;
    }
    Ok(())
}

/// Packed by root, which reads every file of the folder whoever wrote it.
pub(super) async fn copy_out(
    live: &Live,
    guest: &str,
    host: &Utf8Path,
) -> Result<(), SandboxError> {
    let mut entries =
        std::fs::read_dir(host).map_err(|error| backend(format!("{host}: {error}")))?;
    if entries.next().is_some() {
        return Err(backend(format!(
            "{host}: not empty, copy_out takes an empty folder"
        )));
    }
    let tar = format!("{}/{COPY_TAR}", network::NET_DIR);
    let args = ["-c", "-f", &tar, "-C", guest, "."].map(str::to_owned);
    let packed = root(&live.vm, "tar", args.to_vec(), COPY).await?;
    let unpacked = (packed.status().code == 0).then(|| {
        let staged = network::net_dir(&live.folder).join(COPY_TAR);
        std::fs::File::open(&staged).and_then(|file| tar::Archive::new(file).unpack(host))
    });
    // Through the agent, so the guest's view of the bind never keeps a file the host removed.
    let fs = live.vm.fs();
    let _ = within(GUEST_CALL, "remove the tar", fs.remove(&tar)).await;
    match unpacked {
        None => Err(guest_failed(&format!("tar -c of {guest}"), &packed)),
        Some(unpacked) => {
            unpacked.map_err(|error| backend(format!("unpack {guest} into {host}: {error}")))
        }
    }
}

pub(super) async fn root(
    vm: &msb::Sandbox,
    program: &str,
    args: Vec<String>,
    limit: Duration,
) -> Result<msb::ExecOutput, SandboxError> {
    run_for(vm, program, limit, |options| {
        options.args(args).user("root")
    })
    .await
}

/// The SDK kills `program` in the guest at `limit`; the adapter gives up on an SDK that has not
/// answered `MARGIN` later.
async fn run_for(
    vm: &msb::Sandbox,
    program: &str,
    limit: Duration,
    options: impl FnOnce(ExecOptionsBuilder) -> ExecOptionsBuilder,
) -> Result<msb::ExecOutput, SandboxError> {
    let call = vm.exec_with(program, |given| options(given).timeout(limit));
    within(limit + MARGIN, program, call)
        .await?
        .map_err(exec_error)
}

/// `Ok(None)` once `file` shows on the host; the job's state when it ended without it.
pub(super) async fn wait_for_file(
    job: &Job,
    file: &Utf8Path,
) -> Result<Option<JobInfo>, SandboxError> {
    let look = || async move {
        within(GUEST_CALL, "inspect a job", job.inspect())
            .await?
            .map_err(backend)
            .map(|info| (!info.state.is_active()).then_some(info))
    };
    wait_for(file, look).await
}

/// `look` gives the job's state once the job has ended.
async fn wait_for<T, F: Future<Output = Result<Option<T>, SandboxError>>>(
    file: &Utf8Path,
    mut look: impl FnMut() -> F,
) -> Result<Option<T>, SandboxError> {
    let deadline = Instant::now() + GUEST_CALL;
    loop {
        if file.exists() {
            return Ok(None);
        }
        if let Some(state) = look().await? {
            // A quick job can write the file and end between the two looks; the guest renames it
            // into place on the host, so a job seen ended has its file there if it wrote one.
            return Ok((!file.exists()).then_some(state));
        }
        if Instant::now() >= deadline {
            return Err(guest_error(format!(
                "no {file} within {} s",
                GUEST_CALL.as_secs()
            )));
        }
        tokio::time::sleep(POLL).await;
    }
}

/// What the service wrote on stdout and stderr, whole, as `run` keeps it in the VM's run folder.
pub(super) fn service_log(live: &Live) -> Result<Vec<u8>, SandboxError> {
    let path = run_dir(&live.folder).join(LOG);
    std::fs::read(&path).map_err(|error| backend(format!("{path}: {error}")))
}

/// What a job of `stand-guest` itself said, from the agent's record of it, which keeps the last
/// MiB: enough for why it failed.
async fn log(job: &Job) -> Result<Vec<u8>, SandboxError> {
    let entries = within(
        GUEST_CALL,
        "read a log",
        job.logs(&JobLogOptions::default()),
    )
    .await?
    .map_err(backend)?;
    Ok(entries.into_iter().flat_map(|entry| entry.data).collect())
}

/// A job of `stand-guest` that ended when it should not have, with what it said.
pub(super) async fn ended(job: &Job, what: &str, info: &JobInfo) -> SandboxError {
    let log = log(job).await.unwrap_or_default();
    let failure = info
        .failure
        .as_ref()
        .map(|failure| failure.message.as_str());
    let why = failure
        .or(info.error.as_deref())
        .map(str::to_owned)
        .unwrap_or_else(|| String::from_utf8_lossy(&log).trim().to_owned());
    guest_error(format!(
        "{what} ended ({:?}, exit {:?}): {why}",
        info.state, info.exit_code
    ))
}

pub(super) fn guest_failed(what: &str, output: &msb::ExecOutput) -> SandboxError {
    guest_error(format!(
        "{what} exited with {}: {}",
        output.status().code,
        String::from_utf8_lossy(output.stderr_bytes()).trim()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(entrypoint: Option<&[&str]>, command: Option<&[&str]>) -> Launch {
        let owned = |list: &[&str]| list.iter().map(|item| item.to_string()).collect();
        Launch {
            service: "db".into(),
            replica: 1,
            image: crate::ImageRef::new("postgres:17-alpine"),
            cpus: 1,
            memory: 512 << 20,
            env: Default::default(),
            workdir: None,
            entrypoint: entrypoint.map(owned),
            command: command.map(owned),
            mounts: Vec::new(),
            ports: Vec::new(),
            peers: Vec::new(),
            outbound: crate::Outbound::Off,
            stop_signal: None,
            stop_grace: Duration::from_secs(10),
            max_duration: Duration::from_secs(600),
            seed: None,
        }
    }

    #[test]
    fn a_probe_output_maps_through_the_deepest_writable_bind() {
        let bind = |host: &str, guest: &str, read_only| Mount::Bind {
            host: host.into(),
            guest: guest.into(),
            read_only,
        };
        let mounts = [
            bind("/run/1", "/stand/out", false),
            bind("/run/1/inspect", "/stand/inspect", false),
            bind("/run/1/preload", "/stand/preload", true),
        ];
        let mapped = |stdout: &str| guest_path(&mounts, Utf8Path::new(stdout));
        let deep = mapped("/run/1/inspect/db/1.out").unwrap();
        assert_eq!(deep, "/stand/inspect/db/1.out");
        let shallow = mapped("/run/1/preload/p.out").unwrap();
        assert_eq!(shallow, "/stand/out/preload/p.out");
        for outside in ["/run/2/p.out", "/run/1", "/run/1/../2/p.out"] {
            assert!(
                matches!(mapped(outside), Err(SandboxError::Guest { .. })),
                "{outside}"
            );
        }
    }

    #[test]
    fn a_job_that_writes_its_file_and_ends_between_two_looks_started() {
        let temp = tempfile::tempdir().unwrap();
        let file = &Utf8PathBuf::from_path_buf(temp.path().join(STATUS)).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let wait = |write: bool| {
            let look = move || async move {
                if write {
                    std::fs::write(file, "{}").unwrap();
                }
                Ok(Some(()))
            };
            rt.block_on(wait_for(file, look)).unwrap()
        };
        assert_eq!(wait(false), Some(()));
        assert_eq!(wait(true), None);
    }

    #[test]
    fn a_stop_signal_of_the_image_takes_the_name_stand_guest_sends() {
        let named = ["SIGINT", "quit", "SigTerm", " WINCH ", "3"].map(docker_signal_name);
        assert_eq!(named, ["SIGINT", "SIGQUIT", "SIGTERM", "SIGWINCH", "3"]);
    }

    #[test]
    fn id_gives_the_uid_and_gid_of_gnu_and_busybox() {
        let owner = |uid, gid| Some(Owner { uid, gid });
        let gnu = "uid=1000(node) gid=1000(node) groups=1000(node)\n";
        assert_eq!(parse_id(gnu), owner(1000, 1000));
        let unnamed = "uid=101 gid=0(root) groups=0(root)";
        assert_eq!(parse_id(unnamed), owner(101, 0));
        assert_eq!(parse_id("id: unknown user"), None);
    }

    #[test]
    fn a_new_entrypoint_drops_the_cmd_of_the_image_as_compose_does() {
        let image = ImageDefaults {
            user: String::new(),
            workdir: None,
            stop_signal: "SIGTERM".into(),
            entrypoint: vec!["docker-entrypoint.sh".into()],
            cmd: vec!["postgres".into()],
        };
        let argv = |launch: Launch| command(&launch, &image).unwrap();
        assert_eq!(
            argv(launch(None, None)),
            ["docker-entrypoint.sh", "postgres"]
        );
        assert_eq!(
            argv(launch(None, Some(&["postgres", "-c", "x"]))),
            ["docker-entrypoint.sh", "postgres", "-c", "x"]
        );
        assert_eq!(argv(launch(Some(&["sh"]), None)), ["sh"]);
        assert_eq!(
            argv(launch(Some(&["sh"]), Some(&["-c", "true"]))),
            ["sh", "-c", "true"]
        );
    }
}
