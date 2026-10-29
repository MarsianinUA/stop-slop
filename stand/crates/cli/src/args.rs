//! `stop-slop-stand run [--json [--pretty]] | snapshot | clean [--all]`. `--help` holds at most 20
//! lines, the last ``agents: run `guide` ``.

use std::ffi::OsString;
use std::io::Write;

use clap::{Args, Parser, Subcommand};
use report::{Cause, Failure, Line, Phase};

use crate::{Exit, next};

pub enum Command {
    Run { output: Output },
    Snapshot,
    Clean(store::Clean),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    Summary,
    Json(JsonLayout),
}

/// `--pretty` needs `--json`, as in the linter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonLayout {
    Line,
    Pretty,
}

/// `args` starts with the program's name, as `std::env::args_os` does.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, clap::Error> {
    Cli::try_parse_from(args).map(|cli| cli.command.into_command())
}

/// `--help` and `--version` land here too, as errors that print to stdout with exit 0. Otherwise
/// clap's message goes to stderr, and `run .. --json` also prints its line with the phase `args`,
/// as the linter does: nothing parsed, so `--json` is looked for among the raw arguments.
pub fn refuse_args(
    error: &clap::Error,
    args: &[OsString],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Exit {
    // A stdout or stderr that fails has nowhere left to report it; the exit code still tells.
    let text = error.render().to_string();
    if !error.use_stderr() {
        let _ = write!(out, "{text}");
        return Exit::Passed;
    }
    let _ = write!(err, "{text}");
    let next = vec![report::Next::Guide(None)];
    let _ = next::print_next(err, &next);
    let mut rest = args.iter().skip(1);
    let is_run = rest.next().is_some_and(|command| command == "run");
    if is_run && rest.any(|arg| arg == "--json") {
        // The error runs to the first empty line; usage and tips for a person follow, stderr keeps them.
        let message = text.split("\n\n").next().unwrap_or_default();
        let message = message.strip_prefix("error: ").unwrap_or(message);
        let failure = Failure {
            phase: Phase::Args,
            service: None,
            repeat: None,
            cause: Cause::Error(message.trim().to_owned()),
            logs: None,
            probes: Vec::new(),
        };
        let line = Line::failed(failure, None, next);
        let _ = next::print_line(out, &line, JsonLayout::Line);
    }
    Exit::Refused
}

#[derive(Parser)]
#[command(
    name = "stop-slop-stand",
    bin_name = "stop-slop-stand",
    version,
    about = "Measures your app in microsandbox VMs: the head, and with --before the tree it changed",
    disable_help_subcommand = true,
    after_help = "agents: run `guide`"
)]
struct Cli {
    #[command(subcommand)]
    command: Sub,
}

#[derive(Subcommand)]
enum Sub {
    /// Measure the head in VMs, against --before when given
    Run(RunArgs),
    /// Snapshot the work tree as the before of this session
    Snapshot,
    /// Delete what the stand keeps for its sessions
    Clean {
        /// Everything, the journals and the microsandbox runtime too
        #[arg(long)]
        all: bool,
    },
}

#[derive(Args)]
struct RunArgs {
    /// One line of JSON for an agent
    #[arg(long)]
    json: bool,
    /// Indent the JSON by 2 spaces; needs --json
    #[arg(long, requires = "json")]
    pretty: bool,
}

impl Sub {
    fn into_command(self) -> Command {
        match self {
            Sub::Run(args) => args.into_command(),
            Sub::Snapshot => Command::Snapshot,
            Sub::Clean { all } => Command::Clean(if all {
                store::Clean::All
            } else {
                store::Clean::Sessions
            }),
        }
    }
}

impl RunArgs {
    fn into_command(self) -> Command {
        let output = match (self.json, self.pretty) {
            (false, _) => Output::Summary,
            (true, false) => Output::Json(JsonLayout::Line),
            (true, true) => Output::Json(JsonLayout::Pretty),
        };
        Command::Run { output }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &[&str]) -> Result<Command, clap::Error> {
        let args = std::iter::once("stop-slop-stand").chain(args.iter().copied());
        parse(args.map(OsString::from))
    }

    #[test]
    fn pretty_needs_json_and_clean_all_is_its_own_scope() {
        assert!(parsed(&["run", "--pretty"]).is_err());
        let Ok(Command::Run { output }) = parsed(&["run", "--json", "--pretty"]) else {
            panic!("a run with indented JSON");
        };
        assert_eq!(output, Output::Json(JsonLayout::Pretty));
        let Ok(Command::Run { output }) = parsed(&["run"]) else {
            panic!("a run with the summary");
        };
        assert_eq!(output, Output::Summary);
        let Ok(Command::Clean(what)) = parsed(&["clean", "--all"]) else {
            panic!("clean --all");
        };
        assert_eq!(what, store::Clean::All);
        let Ok(Command::Clean(what)) = parsed(&["clean"]) else {
            panic!("clean");
        };
        assert_eq!(what, store::Clean::Sessions);
    }
}
