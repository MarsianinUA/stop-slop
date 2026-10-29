//! The raw outputs of a stand run into `report.json`, the `--json` line and the summary. `run`
//! returns `Raw`, so this crate sits below `run` (plan, row 85); `cli` adds the `Context`.

use std::sync::Arc;
use std::{fmt, io};

use camino::Utf8Path;

mod behaviour;
mod context;
mod flags;
mod grade;
mod hot;
mod line;
mod measures;
mod modes;
mod next;
mod outputs;
mod raw;
mod schema;
mod stats;
mod summary;

pub use context::{
    BeforeInfo, Bound, Context, Declared, Intent, Journal, PastRun, PostHoc, SealInfo, SetupFrom,
};
pub use flags::{DeadReason, Flag};
pub use next::{Next, Topic};
pub use outputs::{Outputs, read_outputs};
pub use raw::{
    BeforeOutcome, Cause, CoverageOutcome, Extra, Failure, FaultOutcome, FaultRepeat,
    InstanceOutput, Pass, PassPair, Phase, ProbeRan, ProbeRule, ProfileOutcome, Raw, RepeatOutput,
    Role, SecondPassOutcome, Setup, SetupSide,
};
pub use schema::{
    At, BeforeJson, Behaviour, CalledJson, Categories, CategoryChange, CategoryJson, Compared,
    CoverageJson, Diff, Direction, ExpectJson, FaultJson, Grade, Head, Hot, HotVersus, IntentJson,
    JournalJson, Line, LineFault, LineFile, LineMeasure, LinePass, Measure, Need, PassFile,
    PassOpen, PassState, RecipeJson, Report, SealJson, SecondPassJson, SetupJson, Src, Unit,
    Verdict, VersusBefore, VersusPrevious,
};
pub use summary::Summary;

/// The last passed run outside the pool (plan, row 35). `Unreadable` is `previous-unreadable`.
#[derive(Debug, Clone, Copy)]
pub enum Previous<'r> {
    None,
    Unreadable,
    Read(&'r Report),
}

impl<'r> Previous<'r> {
    fn report(self) -> Option<&'r Report> {
        match self {
            Previous::Read(report) => Some(report),
            Previous::None | Previous::Unreadable => None,
        }
    }
}

/// Setup sizes and `after-setup` records of pooled runs with one setup origin (`setup.cached`, else
/// the run) count once: they are one measurement (plan, row 88).
pub fn make(raw: &Raw, outputs: &Outputs, context: &Context<'_>) -> Report {
    let before = Before::of(raw);
    let (measures, mut flags) = measures::of(raw, outputs, context, before);
    flags.extend(grade::unknown_expect(&measures, context));
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
    let (second_pass, failed) = modes::second_pass(raw, outputs, context);
    flags.extend(failed);
    flags.extend(modes::grown(second_pass.as_ref()));
    flags::order(&mut flags);
    let previous = context.previous.report();
    Report {
        schema_version: schema::SCHEMA_VERSION,
        run: context.run.clone(),
        root: schema::slashed(&context.root),
        recipe: RecipeJson {
            digest: raw.digest.clone(),
        },
        seal: context.sealed().map(schema::seal_json),
        created_at: schema::time(context.created_at),
        repeat: raw.repeat,
        setup_timeout: schema::duration(raw.setup_timeout),
        timeout: schema::duration(raw.timeout),
        attempts: u32::try_from(context.pool.len() + 1).unwrap_or(u32::MAX),
        comparisons: context.comparisons(),
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
        needs: grade::needs(&measures),
        measures,
        flags,
        hot: hot::of(raw, before, previous),
        coverage: modes::coverage(raw, context),
        behaviour,
        fault: modes::fault(raw, context),
        second_pass,
        records: Arc::clone(&outputs.records),
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
    #[error(
        "{path}: schemaVersion {found}, this tool writes {}",
        schema::SCHEMA_VERSION
    )]
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

    fn bad(path: &Utf8Path, message: impl fmt::Display) -> ReportError {
        ReportError::Bad {
            path: path.to_owned(),
            message: message.to_string(),
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

impl Report {
    /// The run that built a version's setup: its `setup.cached`, else the run itself (plan, row
    /// 88). `None` for "before" of a run without it.
    fn origin(&self, side: Side) -> Option<&str> {
        let cached = match side {
            Side::Head => self.head.setup.cached.as_deref(),
            Side::Before => self.before.as_ref()?.setup.cached.as_deref(),
        };
        Some(cached.unwrap_or(&self.run))
    }
}

impl Context<'_> {
    /// The seal of a run with `--before`: a plain run is free exploration (plan, row 65).
    fn sealed(&self) -> Option<&SealInfo> {
        self.before.as_ref().and(self.seal.as_ref())
    }

    /// This run and the passed runs with `--before` from the last seal on: a failed run compared
    /// nothing, and a reseal starts the count again.
    fn comparisons(&self) -> Option<u32> {
        self.before.as_ref()?;
        let since = self.sealed().map(|seal| seal.sealed_at);
        let earlier = self.history.iter().filter(|run| {
            run.compared && run.passed && since.is_none_or(|since| run.created_at >= since)
        });
        Some(u32::try_from(earlier.count() + 1).unwrap_or(u32::MAX))
    }

    fn origin(&self, side: Side) -> &str {
        let cached = match side {
            Side::Head => self.setup_from.head.as_deref(),
            Side::Before => self.setup_from.before.as_deref(),
        };
        cached.unwrap_or(&self.run)
    }
}
