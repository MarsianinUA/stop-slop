//! The command `stop-slop-stand`. `execute` is the whole program but the process: `main` gives it
//! the folders of the OS, the adapter, Ctrl+C and the host, tests give it a `Fake` and their own.
//!
//! A failed write of a refusal, a line or a summary (`let _ =`) is not reported: there is nowhere
//! left to report it, and the exit code still tells.

use std::io::Write;
use std::process::ExitCode;
use std::time::SystemTime;

use budget::{Host, HostError};
use camino::{Utf8Path, Utf8PathBuf};
use report::{Cause, Failure, Line, Next, Phase, slashed};
use run::Cancel;
use sandbox::Sandbox;
use store::{RunLock, Session, Store};

use crate::error::{At, Halt};

mod accept;
mod args;
mod cache;
mod clean;
mod context;
mod error;
mod guide;
mod inspect;
mod lock;
mod measure;
mod next;
mod seal;
mod snapshot;
mod sweep;

pub use args::{Command, Compare, JsonLayout, Output, Reason, parse, refuse_args};

use error::StandError;

/// The name of the stand file in its folder, `<git common>/info/stop-slop-stand/` (plan, row 52).
const STAND_FILE: &str = "compose.yaml";

pub struct Env<'a> {
    pub cwd: &'a Utf8Path,
    pub store: &'a Store,
    pub cancel: &'a Cancel,
    /// `budget::Host::read` in the binary. Tests give a host of their own: a CI runner has two
    /// cores and less than the 20 GB of disk the stand keeps free.
    pub host: &'a dyn Fn(&Utf8Path) -> Result<Host, HostError>,
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Passed = 0,
    AppFailed = 1,
    Refused = 2,
    Broken = 3,
    Cancelled = 130,
}

impl From<Exit> for ExitCode {
    fn from(exit: Exit) -> ExitCode {
        ExitCode::from(exit as u8)
    }
}

pub fn execute(command: Command, sandbox: &impl Sandbox, mut env: Env<'_>) -> Exit {
    let done = match command {
        Command::Run { compare, output } => {
            return measure::run(compare.as_ref(), output, sandbox, &mut env);
        }
        Command::DryRun { before, output } => {
            return measure::dry(before.as_ref(), output, sandbox, &mut env);
        }
        Command::Snapshot => snapshot::run(&mut env),
        Command::Inspect {
            run,
            side,
            service,
            argv,
        } => inspect::run(&run, side, &service, &argv, sandbox, &mut env),
        Command::Accept {
            measure,
            allow,
            reason,
        } => accept::run(&measure, &allow, &reason, &mut env),
        Command::Guide { topic } => guide::run(topic, &mut env),
        Command::Init => guide::init(&mut env),
        Command::Clean(what) => clean::run(what, sandbox, &mut env),
    };
    done.unwrap_or_else(|error| refused(&error, &mut env))
}

/// What fails before `execute`: the folders of the OS, the stand folder, the adapter. Exit 3, and
/// `run --json` prints its line too, with the phase `config`: every exit code has one (row 83).
pub fn refuse_start(
    command: &Command,
    message: &str,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Exit {
    let next = vec![Next::Guide(None)];
    let _ = writeln!(err, "{message}");
    let _ = next::print_next(err, &next);
    if let Command::Run { output, .. } | Command::DryRun { output, .. } = command
        && let Output::Json(layout) = *output
    {
        let line = refusal_line(Phase::Config, error_cause(message), next);
        let _ = next::print_line(out, &line, layout);
    }
    Exit::Broken
}

fn refusal_line(phase: Phase, cause: Cause, next: Vec<Next>) -> Line {
    let failure = Failure {
        phase,
        service: None,
        repeat: None,
        cause,
        logs: None,
        probes: Vec::new(),
    };
    Line::failed(failure, None, next)
}

fn error_cause(text: &str) -> Cause {
    Cause::Error(text.lines().collect::<Vec<_>>().join("; "))
}

/// The commands without `--json`: the reason, then on 2 and 3 the `next:` lines (plan, row 83).
fn refused(error: &StandError, env: &mut Env<'_>) -> Exit {
    let _ = writeln!(env.err, "{error}");
    let exit = error.exit();
    if matches!(exit, Exit::Refused | Exit::Broken) {
        let _ = next::print_next(env.err, &error.next());
    }
    exit
}

/// `<git common>/info/stop-slop-stand/compose.yaml`. `spec::load` makes a relative path absolute
/// by the process's cwd, so the path starts from `cwd` (git gives `common` absolute anyway).
fn stand_file(cwd: &Utf8Path, worktree: &source::Worktree) -> Utf8PathBuf {
    cwd.join(&worktree.common)
        .join("info")
        .join("stop-slop-stand")
        .join(STAND_FILE)
}

/// What `run`, `inspect` and `accept` read before the lock: the work tree, `stand.toml` and the
/// live stand file. The key of a session is the stand file's path and `session()` rewrites the
/// session's images, so the stand file must load, and the lock waits its `run::span` (Gate 1,
/// row 65).
struct Opened {
    worktree: source::Worktree,
    file: Utf8PathBuf,
    policy: spec::Policy,
    share: budget::Share,
    live: spec::Stand,
}

impl Opened {
    fn read(env: &Env<'_>) -> Result<Opened, Halt> {
        let worktree = source::open(env.cwd).at(Phase::Source)?;
        let (policy, share) = policy(env.store).at(Phase::Config)?;
        let file = stand_file(env.cwd, &worktree);
        let live = spec::load(&file, &policy).at(Phase::Spec)?;
        Ok(Opened {
            worktree,
            file,
            policy,
            share,
            live,
        })
    }

    fn lock(&self, env: &mut Env<'_>) -> Result<RunLock, Halt> {
        let span = run::span(&self.live);
        lock::wait(env.store, span, env.cancel, &mut *env.err).at(Phase::Lock)
    }

    /// The caller keeps the lock while it uses the session.
    fn session(
        &self,
        env: &mut Env<'_>,
        now: SystemTime,
    ) -> Result<(RunLock, Session), StandError> {
        let lock = self.lock(env)?;
        let images = images(&self.live);
        let session = env
            .store
            .session(&self.file, &self.worktree.root, &images, now)?;
        Ok((lock, session))
    }
}

/// The images of a stand, each once, in the order of their services.
fn images(stand: &spec::Stand) -> Vec<String> {
    let mut images: Vec<String> = Vec::new();
    for service in stand.services.values() {
        if !images.contains(&service.image) {
            images.push(service.image.clone());
        }
    }
    images
}

/// `max_tolerance` and `host_share` of `stand.toml`, each refusal with its key (plan, row 66).
fn policy(store: &Store) -> Result<(spec::Policy, budget::Share), StandError> {
    let config = store.config()?;
    let file = &store.paths().config;
    let refused = |key: &str, message: String| store::ConfigError {
        file: file.clone(),
        key: key.to_owned(),
        message,
    };
    let policy = spec::Policy::new(config.max_tolerance)
        .map_err(|error| refused("max_tolerance", error.to_string()))?;
    let share = budget::Share::new(config.host_share)
        .map_err(|error| refused("host_share", error.to_string()))?;
    Ok((policy, share))
}
