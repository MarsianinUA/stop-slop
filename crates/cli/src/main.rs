mod args;
mod check;
mod error;
mod hook;
mod ignore;
mod list;
mod output;
mod toggle;

use std::collections::BTreeSet;
use std::env;
use std::process::{self, ExitCode};

use camino::Utf8PathBuf;
use clap::Parser;
use engine::{Engine, RunConfig};
use kit::Ruleset;
use repo::{Repo, RepoError};
use state::State;

use args::Command;
use error::{CliError, Exit};
use output::JsonLayout;

fn main() -> ExitCode {
    // A handler that cannot be set leaves Ctrl+C as it was: the signal ends the process.
    let _ = ctrlc::set_handler(|| process::exit(Exit::Interrupted as i32));
    let exit = match Command::try_parse() {
        Ok(command) => run(command),
        Err(error) => refuse_args(&error),
    };
    ExitCode::from(exit as u8)
}

fn run(command: Command) -> Exit {
    let json = wants_json(&command);
    let is_hook = matches!(command, Command::Hook);
    let result = match command {
        Command::Check(args) => check::run(args),
        Command::Hook => hook::run(),
        Command::Ignore {
            fingerprint,
            reason,
            base,
        } => ignore::run(fingerprint, reason, base),
        Command::Enable => toggle::enable(),
        Command::Disable => toggle::disable(),
        Command::Rules { json, pretty } => list::run(json, JsonLayout::new(pretty)),
    };
    result.unwrap_or_else(|error| {
        let cause = error.to_string();
        let next = error.next();
        output::print_err(&output::failure_text(&cause, &next));
        if json {
            print_failure(error.phase(), &cause, &next);
        }
        // The agent reads a hook's stderr only on 2.
        if is_hook { Exit::Refused } else { error.exit() }
    })
}

fn wants_json(command: &Command) -> bool {
    match command {
        Command::Check(args) => args.json,
        Command::Rules { json, .. } => *json,
        Command::Hook | Command::Ignore { .. } | Command::Enable | Command::Disable => false,
    }
}

/// `--help` and `--version` come here too, as errors that print to stdout with code 0.
fn refuse_args(error: &clap::Error) -> Exit {
    // An error printing the message has nowhere to be reported.
    let _ = error.print();
    if !error.use_stderr() {
        return Exit::Clean;
    }
    // Nothing parsed, so `--json` is looked for among the raw arguments.
    if env::args_os().skip(1).any(|arg| arg == "--json") {
        // The first line is the error; usage and tips for a person follow it on stderr.
        let text = error.to_string();
        let first = text.lines().next().unwrap_or_default();
        print_failure("args", first.strip_prefix("error: ").unwrap_or(first), &[]);
    }
    Exit::Refused
}

fn print_failure(phase: &'static str, cause: &str, next: &[String]) {
    // A stdout that fails here has nowhere left to report it; the exit code still tells.
    let _ = output::print(&output::failure_json(phase, cause, next));
}

fn cwd_repo() -> Result<Repo, CliError> {
    let cwd = env::current_dir().map_err(|source| RepoError::Io {
        path: ".".into(),
        source,
    })?;
    Ok(Repo::open(&cwd)?)
}

fn new_engine(
    set: &'static Ruleset,
    state: &State,
    ignored: BTreeSet<(Utf8PathBuf, String)>,
) -> Result<Engine, CliError> {
    let config = state.config()?;
    let config = RunConfig {
        disabled: config.disabled,
        enabled: config.enabled,
        options: config.rules,
        ignored,
    };
    Engine::new(set, config).map_err(|source| CliError::Config {
        path: state.config_path(),
        source,
    })
}

/// With the exceptions of `ignore.json`.
fn engine_for(set: &'static Ruleset, state: &State) -> Result<Engine, CliError> {
    let ignored = state
        .ignores()?
        .into_iter()
        .map(|entry| (entry.file, entry.fingerprint))
        .collect();
    new_engine(set, state, ignored)
}
