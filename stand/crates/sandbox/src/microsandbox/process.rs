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

use super::{GUEST_CALL, Live, POLL, backend, guest, within};
use crate::{ExecOutcome, ImageRef, Launch, Mount, SandboxError, Stopped};

/// Where `run` keeps its status file: the service's user writes it, so it is a bind of its own and
/// `/stand/net`, which root writes, stays out of that user's reach.
pub(super) const RUN_DIR: &str = "/stand/run";
const STATUS: &str = "status.json";
/// Room for `stand-guest` to act at its own deadline and report before the SDK gives up.
const MARGIN: Duration = Duration::from_secs(10);
/// Outside any folder a service would ask for, and gone right after the copy.
const COPY_TAR: &str = "/tmp/stop-slop-stand-copy.tar";
const STOP_SIGNAL: &str = "SIGTERM";
const COPY: Duration = Duration::from_secs(300);

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
        .filter(|signal| !signal.is_empty()))
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

/// Healthcheck attempts, as the image's user with the VM's env.
pub(super) async fn exec_detached(live: &Live, argv: &[String]) -> Result<Job, SandboxError> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| backend("an empty healthcheck command"))?;
    let args = args.to_vec();
    let start = live.vm.exec_detached_with(program.clone(), |options| {
        with_env(options.args(args).stdin_null(), &live.launch, &live.image)
    });
    within(GUEST_CALL, "start a healthcheck", start)
        .await?
        .map_err(backend)
}

pub(super) async fn stop(live: &Live) -> Result<Stopped, SandboxError> {
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
    let call = live.vm.exec_with(guest::BIN, |options| {
        let options = options.args(args).user("root").timeout(left + MARGIN);
        with_env(options, &live.launch, &live.image)
    });
    let output = within(left + MARGIN + MARGIN, "run a probe", call)
        .await?
        .map_err(backend)?;
    if output.status().code != 0 {
        return Err(guest_failed("stand-guest exec", &output));
    }
    let path = Utf8PathBuf::from(format!("{stdout}.status"));
    let read = std::fs::read_to_string(&path);
    let _ = std::fs::remove_file(&path);
    let text = read.map_err(|error| guest_error(format!("{path}: {error}")))?;
    let status =
        ExecStatus::parse(&text).map_err(|error| guest_error(format!("{path}: {error}")))?;
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

/// uid and gid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Owner(u32, u32);

/// Whom the agent runs an exec as by default, the image's user: `id` resolves it in the guest as
/// the service's processes see it, whatever form `USER` has in the image.
pub(super) async fn owner(vm: &msb::Sandbox) -> Result<Owner, SandboxError> {
    let call = vm.exec_with("id", |options| options.timeout(GUEST_CALL));
    let output = within(GUEST_CALL + MARGIN, "id", call)
        .await?
        .map_err(backend)?;
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
    Some(Owner(number("uid=")?, number("gid=")?))
}

/// Through the agent, which is root.
pub(super) async fn give(
    vm: &msb::Sandbox,
    path: &str,
    Owner(uid, gid): Owner,
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
    let args = ["-c", "-f", COPY_TAR, "-C", guest, "."].map(str::to_owned);
    let packed = root(&live.vm, "tar", args.to_vec(), COPY).await?;
    if packed.status().code != 0 {
        return Err(guest_failed(&format!("tar -c of {guest}"), &packed));
    }
    let staged = live.folder.join("copy.tar");
    let fs = live.vm.fs();
    let copied = within(COPY, "copy a tar out", fs.copy_to_host(COPY_TAR, &staged)).await;
    let _ = within(GUEST_CALL, "remove the copied tar", fs.remove(COPY_TAR)).await;
    copied?.map_err(backend)?;
    let unpacked = std::fs::File::open(&staged).and_then(|tar| tar::Archive::new(tar).unpack(host));
    let _ = std::fs::remove_file(&staged);
    unpacked.map_err(|error| backend(format!("unpack {guest} into {host}: {error}")))
}

pub(super) async fn root(
    vm: &msb::Sandbox,
    program: &str,
    args: Vec<String>,
    limit: Duration,
) -> Result<msb::ExecOutput, SandboxError> {
    let call = vm.exec_with(program, |options| {
        options.args(args).user("root").timeout(limit)
    });
    within(limit + MARGIN, program, call)
        .await?
        .map_err(backend)
}

/// `Ok(None)` once `file` shows on the host; the job's state when it ended without it.
pub(super) async fn wait_for_file(
    job: &Job,
    file: &Utf8Path,
) -> Result<Option<JobInfo>, SandboxError> {
    let deadline = Instant::now() + GUEST_CALL;
    loop {
        if file.exists() {
            return Ok(None);
        }
        let info = within(GUEST_CALL, "inspect a job", job.inspect())
            .await?
            .map_err(backend)?;
        if !info.state.is_active() {
            return Ok(Some(info));
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

/// A job of `stand-guest` that ended when it should not have, with what it said.
pub(super) async fn ended(job: &Job, what: &str, info: &JobInfo) -> SandboxError {
    let read = within(
        GUEST_CALL,
        "read a log",
        job.logs(&JobLogOptions::default()),
    )
    .await;
    let log: Vec<u8> = match read {
        Ok(Ok(entries)) => entries.into_iter().flat_map(|entry| entry.data).collect(),
        _ => Vec::new(),
    };
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

fn guest_failed(what: &str, output: &msb::ExecOutput) -> SandboxError {
    guest_error(format!(
        "{what} exited with {}: {}",
        output.status().code,
        String::from_utf8_lossy(output.stderr_bytes()).trim()
    ))
}

fn guest_error(message: String) -> SandboxError {
    SandboxError::Guest { message }
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
    fn id_gives_the_uid_and_gid_of_gnu_and_busybox() {
        let gnu = "uid=1000(node) gid=1000(node) groups=1000(node)\n";
        assert_eq!(parse_id(gnu), Some(Owner(1000, 1000)));
        let unnamed = "uid=101 gid=0(root) groups=0(root)";
        assert_eq!(parse_id(unnamed), Some(Owner(101, 0)));
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
