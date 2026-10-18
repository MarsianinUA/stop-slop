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
use std::process::ExitCode;

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
    let result = match Command::parse() {
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
    let exit = result.unwrap_or_else(|error| {
        eprintln!("stop-slop: {error}");
        Exit::Failure
    });
    ExitCode::from(exit as u8)
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
