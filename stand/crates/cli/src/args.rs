//! `stop-slop-stand run [--before session|<ref> [--reseal --reason <text>]] [--dry-run] [--json
//! [--pretty]] | snapshot | inspect <run> <side> <service> -- <argv>... | accept <measure> --allow
//! <bound> --reason <text> | guide [topic] | init | clean [--all]`. `--help` holds at most 20 lines,
//! the last ``agents: run `guide` ``.
//!
//! The types say what the flags cannot be: `--reseal` lives only in `Compare`, which only
//! `run --before` makes, and `DryRun` has no `reseal` (plan, row 9).

use std::ffi::OsString;
use std::io::Write;

use clap::{Args, Parser, Subcommand};
use report::{Cause, Phase, Topic};
use run::Side;
use source::Before;

use crate::{Exit, guide, next, refusal_line};

pub enum Command {
    Run {
        compare: Option<Compare>,
        output: Output,
    },
    /// `run --dry-run`: no VM, no pull, no seal.
    DryRun {
        before: Option<Before>,
        output: Output,
    },
    Inspect {
        run: String,
        side: Side,
        service: String,
        argv: Vec<String>,
    },
    /// `clap` requires both `allow` and `reason`: one without the other is exit 2.
    Accept {
        measure: String,
        allow: String,
        reason: Reason,
    },
    Clean(store::Clean),
    Local(Local),
}

/// The commands that read and write only the work tree and stdout. `main` runs them before it
/// opens the stand folder and starts the adapter, so they work where those fail, and `guide` is
/// the `next` of such a failure (batch review 6).
pub enum Local {
    Snapshot,
    Guide { topic: Option<Topic> },
    Init,
}

/// `run --before <..>`; `--reseal` exists only here and needs `--reason`.
pub struct Compare {
    pub before: Before,
    pub reseal: Option<Reason>,
}

/// A justification with some text in it, the flag name of `stop-slop ignore --reason`.
#[derive(Debug, Clone)]
pub struct Reason(String);

impl Reason {
    pub fn as_str(&self) -> &str {
        &self.0
    }
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
        let cause = Cause::Error(message.trim().to_owned());
        let line = refusal_line(Phase::Args, cause, next);
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
    /// Run a command in a copy of a kept end-of-run snapshot
    Inspect {
        /// The id of the kept run, `run` of the --json line
        run: String,
        /// head or before
        #[arg(value_parser = side)]
        side: Side,
        /// A service with a volume
        service: String,
        /// The command to run in the copy
        #[arg(last = true, required = true)]
        argv: Vec<String>,
    },
    /// Accept the shift of a measure after the fact, with a reason
    Accept {
        /// A measure of the last passed run, such as stand.wall
        measure: String,
        /// none, +X%, -X% or down, X from 5
        #[arg(long, allow_hyphen_values = true)]
        allow: String,
        /// Why the measure moved
        #[arg(long, value_parser = reason)]
        reason: Reason,
    },
    /// Print the guide for agents, or one of its topics
    Guide {
        /// recipe, probes, expect, line or inspect
        #[arg(value_parser = topic)]
        topic: Option<Topic>,
    },
    /// Write a stand file to start from
    Init,
    /// Delete what the stand keeps for its sessions
    Clean {
        /// Everything, the journals and the microsandbox runtime too
        #[arg(long)]
        all: bool,
    },
}

#[derive(Args)]
struct RunArgs {
    /// `session` for the snapshot of this work tree, or a ref for its merge-base with HEAD
    #[arg(long)]
    before: Option<String>,
    /// Seal the changed recipe again; needs --before and --reason
    #[arg(long, requires_all = ["before", "reason"], conflicts_with = "dry_run")]
    reseal: bool,
    /// Why the recipe changed; the journal of the seal keeps it
    #[arg(long, requires = "reseal", value_parser = reason)]
    reason: Option<Reason>,
    /// Check the stand file, the seal, the budget and the cache; no VM starts
    #[arg(long)]
    dry_run: bool,
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
            Sub::Snapshot => Command::Local(Local::Snapshot),
            Sub::Inspect {
                run,
                side,
                service,
                argv,
            } => Command::Inspect {
                run,
                side,
                service,
                argv,
            },
            Sub::Accept {
                measure,
                allow,
                reason,
            } => Command::Accept {
                measure,
                allow,
                reason,
            },
            Sub::Guide { topic } => Command::Local(Local::Guide { topic }),
            Sub::Init => Command::Local(Local::Init),
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
        let before = self.before.map(|text| match text.as_str() {
            "session" => Before::Session,
            _ => Before::Ref(text),
        });
        if self.dry_run {
            return Command::DryRun { before, output };
        }
        let compare = before.map(|before| Compare {
            before,
            reseal: self.reason,
        });
        Command::Run { compare, output }
    }
}

fn reason(text: &str) -> Result<Reason, String> {
    if text.trim().is_empty() {
        Err("the reason is empty".to_owned())
    } else {
        Ok(Reason(text.to_owned()))
    }
}

fn side(text: &str) -> Result<Side, String> {
    match text {
        "head" => Ok(Side::Head),
        "before" => Ok(Side::Before),
        _ => Err("head or before".to_owned()),
    }
}

fn topic(text: &str) -> Result<Topic, String> {
    guide::TOPICS
        .into_iter()
        .find(|topic| guide::name(*topic) == text)
        .ok_or_else(|| format!("topics: {}", guide::names()))
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
        let Ok(Command::Run { output, .. }) = parsed(&["run", "--json", "--pretty"]) else {
            panic!("a run with indented JSON");
        };
        assert_eq!(output, Output::Json(JsonLayout::Pretty));
        let Ok(Command::Run { output, .. }) = parsed(&["run"]) else {
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

    #[test]
    fn reseal_needs_before_and_a_reason_and_never_goes_with_dry_run() {
        for args in [
            &["run", "--reseal", "--reason", "x"][..],
            &["run", "--before", "session", "--reseal"],
            &["run", "--before", "session", "--reason", "x"],
            &[
                "run",
                "--before",
                "session",
                "--reseal",
                "--reason",
                "x",
                "--dry-run",
            ],
            &["run", "--before", "session", "--reseal", "--reason", " "],
        ] {
            assert!(parsed(args).is_err(), "{args:?}");
        }
        let args = [
            "run",
            "--before",
            "main",
            "--reseal",
            "--reason",
            "new probe",
            "--json",
            "--pretty",
        ];
        let Ok(Command::Run {
            compare: Some(compare),
            output,
        }) = parsed(&args)
        else {
            panic!("a reseal of a run against main");
        };
        assert_eq!(compare.before, Before::Ref("main".into()));
        assert_eq!(
            compare.reseal.map(|reason| reason.0),
            Some("new probe".into())
        );
        assert_eq!(output, Output::Json(JsonLayout::Pretty));
        let Ok(Command::Run {
            compare: Some(compare),
            ..
        }) = parsed(&["run", "--before", "session"])
        else {
            panic!("a run against the session");
        };
        assert!(compare.before == Before::Session && compare.reseal.is_none());
    }

    #[test]
    fn inspect_takes_its_argv_after_two_dashes_and_accept_a_falling_bound() {
        for args in [
            &["inspect", "20260101-000000", "head", "db", "psql"][..],
            &["inspect", "20260101-000000", "head", "db", "--"],
            &["inspect", "20260101-000000", "middle", "db", "--", "psql"],
            &["accept", "stand.wall", "--allow", "+15%"],
            &["accept", "stand.wall", "--reason", "why"],
            &["accept", "stand.wall", "--allow", "+15%", "--reason", ""],
        ] {
            assert!(parsed(args).is_err(), "{args:?}");
        }
        let args = [
            "inspect",
            "20260101-000000",
            "before",
            "db",
            "--",
            "psql",
            "-c",
            "select 1",
        ];
        let Ok(Command::Inspect {
            run,
            side: Side::Before,
            service,
            argv,
        }) = parsed(&args)
        else {
            panic!("an inspect of before");
        };
        assert_eq!((run.as_str(), service.as_str()), ("20260101-000000", "db"));
        assert_eq!(argv, ["psql", "-c", "select 1"]);
        let args = [
            "accept",
            "db.memory",
            "--allow",
            "-10%",
            "--reason",
            "a cache",
        ];
        let Ok(Command::Accept { allow, reason, .. }) = parsed(&args) else {
            panic!("an accept of a fall");
        };
        assert_eq!((allow.as_str(), reason.as_str()), ("-10%", "a cache"));
    }

    #[test]
    fn dry_run_is_its_own_command() {
        let Ok(Command::DryRun {
            before: Some(Before::Session),
            output: Output::Summary,
        }) = parsed(&["run", "--before", "session", "--dry-run"])
        else {
            panic!("a dry run against the session");
        };
        let Ok(Command::DryRun {
            before: None,
            output: Output::Json(JsonLayout::Line),
        }) = parsed(&["run", "--dry-run", "--json"])
        else {
            panic!("a dry run of the head alone");
        };
    }
}
