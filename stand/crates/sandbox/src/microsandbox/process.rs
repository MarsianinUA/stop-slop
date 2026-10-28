//! The service under `stand-guest run`, its `stop`, and healthcheck attempts.
//!
//! A VM gets no env of its own, fresh or restored (a snapshot carries none, plan row 87): the env,
//! the workdir and the command of the `Launch` go into the exec of the service, and the same env
//! and workdir into every later exec of the VM.

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ::guest::{Command, RunArgs, RunStatus, StopArgs, StopReport};
use ::microsandbox as msb;
use camino::{Utf8Path, Utf8PathBuf};
use msb::jobs::{Job, JobInfo, JobLogOptions, JobState};
use msb::sandbox::ExecOptionsBuilder;

use super::{GUEST_CALL, Live, POLL, backend, guest, within};
use crate::{Launch, SandboxError, Stopped};

/// Where `run` keeps its status file: the service's user writes it, so it is a bind of its own and
/// `/stand/net`, which root writes, stays out of that user's reach.
pub(super) const RUN_DIR: &str = "/stand/run";
const STATUS: &str = "status.json";
/// Room for `stand-guest` to act at its own deadline and report before the SDK gives up.
const MARGIN: Duration = Duration::from_secs(10);

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
    pub workdir: Option<String>,
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
}

pub(super) async fn image_defaults(launch: &Launch) -> Result<ImageDefaults, SandboxError> {
    let detail = msb::Image::inspect(launch.image.as_str())
        .await
        .map_err(backend)?;
    let config = detail.config;
    Ok(ImageDefaults {
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
        signal: live.launch.stop_signal.clone(),
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
            stop_signal: "SIGTERM".into(),
            stop_grace: Duration::from_secs(10),
            max_duration: Duration::from_secs(600),
            seed: None,
        }
    }

    #[test]
    fn a_new_entrypoint_drops_the_cmd_of_the_image_as_compose_does() {
        let image = ImageDefaults {
            workdir: None,
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
