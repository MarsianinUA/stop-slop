//! The command `stop-slop-stand`. `execute` is the whole program but the process: `main` gives it
//! the folders of the OS, the adapter, Ctrl+C and the host, tests give it a `Fake` and their own.

use std::io::Write;
use std::process::ExitCode;
use std::time::SystemTime;

use budget::{Host, HostError};
use camino::{Utf8Path, Utf8PathBuf};
use run::Cancel;
use sandbox::Sandbox;
use store::{RunLock, Session, Store};

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

/// The commands without `--json`: the reason, then on 2 and 3 the `next:` lines (plan, row 83).
fn refused(error: &StandError, env: &mut Env<'_>) -> Exit {
    // A stderr that fails has nowhere left to report it; the exit code still tells.
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

/// The stand file of the work tree, loaded, for `inspect` and `accept`: the key of their session
/// is its path and `session()` rewrites the session's images, so it must load, and they wait its
/// `run::span` (Gate 1, row 65).
struct Loaded {
    worktree: source::Worktree,
    file: Utf8PathBuf,
    policy: spec::Policy,
    stand: spec::Stand,
}

impl Loaded {
    fn read(env: &Env<'_>) -> Result<Loaded, StandError> {
        let worktree = source::open(env.cwd)?;
        let (policy, _) = policy(env.store)?;
        let file = stand_file(env.cwd, &worktree);
        let stand = spec::load(&file, &policy)?;
        Ok(Loaded {
            worktree,
            file,
            policy,
            stand,
        })
    }

    /// The caller keeps the lock while it uses the session.
    fn session(
        &self,
        env: &mut Env<'_>,
        now: SystemTime,
    ) -> Result<(RunLock, Session), StandError> {
        let span = run::span(&self.stand);
        let lock = lock::wait(env.store, span, env.cancel, &mut *env.err)?;
        let images = images(&self.stand);
        let session = env
            .store
            .session(&self.file, &self.worktree.root, &images, now)?;
        Ok((lock, session))
    }
}

/// Paths print with `/` on every OS (plan, row 45).
fn slashed(path: &Utf8Path) -> String {
    path.as_str().replace('\\', "/")
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
