//! The raw outputs of a stand run into `report.json`, the `--json` line and the summary. `run`
//! returns `Raw`, so this crate sits below `run` (plan, row 85); `cli` adds the `Context`.

use std::io;

use camino::Utf8Path;

mod behaviour;
mod context;
mod flags;
mod hot;
mod measures;
mod next;
mod outputs;
mod raw;
mod schema;
mod stats;

pub use context::{BeforeInfo, Context, Intent, SetupFrom};
pub use flags::{DeadReason, Flag};
pub use next::{Next, Topic};
pub use outputs::{Outputs, read_outputs};
pub use raw::{
    BeforeOutcome, Cause, CoverageOutcome, Extra, Failure, FaultOutcome, FaultRepeat,
    InstanceOutput, Pass, PassPair, Phase, ProbeRan, ProbeRule, ProfileOutcome, Raw, RepeatOutput,
    Role, SecondPassOutcome, Setup, SetupSide,
};
pub use schema::{
    At, BeforeJson, Behaviour, CalledJson, CoverageJson, Diff, Direction, Head, Hot, HotVersus,
    IntentJson, Measure, RecipeJson, Report, SetupJson, Src, Unit, Verdict, VersusBefore,
    VersusPrevious,
};

/// The last passed run outside the pool (plan, row 35). `Unreadable` is `previous-unreadable`.
#[derive(Debug, Clone, Copy)]
pub enum Previous<'r> {
    None,
    Unreadable,
    Read(&'r Report),
}

/// Setup sizes and `after-setup` records of pooled runs with one setup origin (`setup.cached`, else
/// the run) count once: they are one measurement (plan, row 88).
pub fn make(raw: &Raw, outputs: &Outputs, context: &Context<'_>) -> Report {
    let before = Before::of(raw);
    let (measures, mut flags) = measures::of(raw, outputs, context, before);
    flags.extend(flags::facts(raw));
    flags.extend(flags::of_context(raw, context));
    flags.extend(flags::bad_lines(outputs));
    flags.extend(behaviour::dead(raw, outputs));
    let behaviour = match before {
        Before::Passed(_) => {
            let (files, file_flags) = behaviour::of(raw, outputs, context);
            flags.extend(file_flags);
            Some(files)
        }
        Before::Absent | Before::Failed(_) => None,
    };
    flags::order(&mut flags);
    let previous = match context.previous {
        Previous::Read(report) => Some(report),
        Previous::None | Previous::Unreadable => None,
    };
    Report {
        schema_version: schema::SCHEMA_VERSION,
        run: context.run.clone(),
        root: schema::slashed(&context.root),
        recipe: RecipeJson {
            digest: raw.digest.clone(),
        },
        created_at: schema::time(context.created_at),
        repeat: raw.repeat,
        setup_timeout: schema::duration(raw.setup_timeout),
        timeout: schema::duration(raw.timeout),
        harness: raw.harness.clone(),
        head: Head {
            tree: context.head_tree.clone(),
            setup: SetupJson {
                cached: context.setup_from.head.clone(),
            },
        },
        before: context
            .before
            .as_ref()
            .map(|info| schema::before_json(info, before, &context.setup_from)),
        previous_run: previous.map(|report| report.run.clone()),
        measures,
        flags,
        hot: hot::of(raw, before, previous),
        coverage: hot::coverage(raw),
        behaviour,
        records: outputs.records.clone(),
    }
}

/// Reads `<dir>/report.json`; another `schemaVersion` is `ReportError::Schema`.
pub fn load(dir: &Utf8Path) -> Result<Report, ReportError> {
    schema::load(dir)
}

#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    #[error("{path}: {source}")]
    Io {
        path: camino::Utf8PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path}: {message}")]
    Bad {
        path: camino::Utf8PathBuf,
        message: String,
    },
    #[error("{path}: schemaVersion {found}, this tool writes 1")]
    Schema {
        path: camino::Utf8PathBuf,
        found: u64,
    },
}

impl ReportError {
    fn io(path: &Utf8Path) -> impl Fn(io::Error) -> ReportError + '_ {
        move |source| ReportError::Io {
            path: path.to_owned(),
            source,
        }
    }
}

/// The "before" version of `Raw` as the report reads it.
#[derive(Debug, Clone, Copy)]
enum Before<'r> {
    /// No `--before`.
    Absent,
    /// Nothing to compare: verdicts against it are `no-baseline` (plan, row 16).
    Failed(&'r Failure),
    Passed(&'r [RepeatOutput]),
}

impl<'r> Before<'r> {
    fn of(raw: &'r Raw) -> Before<'r> {
        match &raw.before {
            None => Before::Absent,
            Some(BeforeOutcome::Failed(failure)) => Before::Failed(failure),
            Some(BeforeOutcome::Passed(repeats)) => Before::Passed(repeats),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Side {
    Head,
    Before,
}

impl Side {
    fn as_str(self) -> &'static str {
        match self {
            Side::Head => "head",
            Side::Before => "before",
        }
    }
}

/// The versions the report reads: the head, and "before" when it passed. A failed "before" has
/// nothing to compare, so its setup outputs are not read either: the `on-failure` probes there are
/// part of its failure (T-12 stage 5 note).
fn sides(raw: &Raw) -> Vec<(Side, &SetupSide, &[RepeatOutput])> {
    let mut sides = vec![(Side::Head, &raw.setup.head, raw.head.as_slice())];
    if let (Before::Passed(repeats), Some(setup)) = (Before::of(raw), &raw.setup.before) {
        sides.push((Side::Before, setup, repeats));
    }
    sides
}

/// The run that built a version's setup: its `setup.cached`, else the run itself (plan, row 88).
fn origin<'a>(cached: Option<&'a str>, run: &'a str) -> &'a str {
    cached.unwrap_or(run)
}
