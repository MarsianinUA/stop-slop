//! `run`: the call order of the plan («Команды `stop-slop-stand`», steps 1-8). Every step tags
//! its error with its phase; whatever happens after the run folder exists, the cache slots, the
//! kept run and the tar files are settled under the lock before it goes.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::Write;
use std::time::SystemTime;

use camino::{Utf8Path, Utf8PathBuf};
use report::{Cause, Extra, Failure, Line, Next, Phase, Previous, Report, Topic};
use run::{Cancel, Failed, RunError, Version, Versions};
use sandbox::{ImageRef, Sandbox};
use source::{Tree, Worktree};
use spec::Stand;
use store::{PoolKey, RunPaths, Session, Slot, Store};

use crate::cache::{self, Planned};
use crate::context::{self, Known};
use crate::error::{At, Halt, StandError};
use crate::{Env, Exit, Output, images, lock, next, policy, stand_file, sweep};

/// How a run ended, for the line or the summary.
enum Ended {
    Passed {
        run: RunPaths,
        report: Box<Report>,
    },
    /// Exit 1: the head failed.
    Failed {
        run: RunPaths,
        failed: Box<Failed>,
    },
    /// Exit 2, 3 or 130; `run` once the run has its folder.
    Halted {
        halt: Halt,
        run: Option<String>,
    },
}

/// Step 1: what is read before the lock.
struct Opened {
    worktree: Worktree,
    file: Utf8PathBuf,
    share: budget::Share,
    live: Stand,
}

/// What the cleanup after the run needs, as far as the run got.
#[derive(Default)]
struct Made {
    session: Option<Session>,
    run: Option<RunPaths>,
    plans: Vec<Planned>,
}

enum Done {
    Passed(Box<Report>),
    Failed(Box<Failed>),
}

pub(crate) fn run(output: Output, sandbox: &impl Sandbox, env: &mut Env<'_>) -> Exit {
    let ended = match opened(env) {
        Err(halt) => Ended::Halted { halt, run: None },
        Ok(opened) => {
            let span = run::span(&opened.live);
            match lock::wait(env.store, span, env.cancel, &mut *env.err).at(Phase::Lock) {
                Err(halt) => Ended::Halted { halt, run: None },
                Ok(_lock) => {
                    let mut made = Made::default();
                    let outcome = measure(opened, sandbox, env, &mut made);
                    settle(outcome, &made, sandbox, env.store, &mut *env.err)
                }
            }
        }
    };
    print(ended, output, env)
}

fn opened(env: &Env<'_>) -> Result<Opened, Halt> {
    let worktree = source::open(env.cwd).at(Phase::Source)?;
    let (policy, share) = policy(env.store).at(Phase::Config)?;
    let file = stand_file(env.cwd, &worktree);
    let live = spec::load(&file, &policy).at(Phase::Spec)?;
    Ok(Opened {
        worktree,
        file,
        share,
        live,
    })
}

/// Steps 3-8 under the lock, up to the written report.
fn measure(
    opened: Opened,
    sandbox: &impl Sandbox,
    env: &mut Env<'_>,
    made: &mut Made,
) -> Result<Done, Halt> {
    let Opened {
        worktree,
        file,
        share,
        live: stand,
    } = opened;
    let store = env.store;
    let now = SystemTime::now();
    let session = store
        .session(&file, &worktree.root, &images(&stand), now)
        .at(Phase::Seal)?;
    let key = session.key().to_owned();
    made.session = Some(session);
    let session = made.session.as_ref().expect("just set");
    checkpoint(env.cancel, Phase::Seal)?;

    sweep::leftovers(sandbox, store, now).at(Phase::Sweep)?;
    checkpoint(env.cancel, Phase::Sweep)?;
    let host = sweep::make_room(sandbox, store, &key, env.host).at(Phase::Budget)?;
    budget::check(demand(&stand), &host, share).at(Phase::Budget)?;
    let pulled = sandbox.pull(&image_refs(&stand)).at(Phase::Pull)?;
    checkpoint(env.cancel, Phase::Pull)?;

    let trees = store.trees().at(Phase::Source)?;
    let head = source::snapshot(&worktree).at(Phase::Source)?;
    write_tar(&worktree, &head, &trees.head).at(Phase::Source)?;
    checkpoint(env.cancel, Phase::Source)?;

    let pool_key = PoolKey {
        head_tree: head.to_string(),
        before_tree: None,
        digest: stand.recipe.digest.clone(),
    };
    let run = session.new_run(now, &pool_key).at(Phase::Source)?;
    made.run = Some(run.clone());

    let left = sandbox.leftovers().at(Phase::Setup)?;
    let head_key = cache::key(&stand, &head, &pulled);
    let head_plan =
        cache::plan(session, Slot::Head, head_key, &left, &run.id, sandbox).at(Phase::Setup)?;
    made.plans.push(head_plan.clone());

    let pool = pool(session, &pool_key).at(Phase::Report)?;
    let (extras, reused) = extras(&stand, pool.last().map(|(_, report)| report));
    let versions = Versions {
        head: Version {
            tar: trees.head.clone(),
            setup: head_plan.setup(),
        },
        before: None,
    };
    let raw = match run::run(&stand, sandbox, &versions, &extras, &run.dir, env.cancel) {
        Ok(raw) => raw,
        Err(RunError::App(failed)) => return Ok(Done::Failed(failed)),
        Err(RunError::Stand { phase, fault }) => return Err(StandError::Run(fault)).at(phase),
        Err(RunError::Cancelled { phase }) => return Err(StandError::Cancelled).at(phase),
    };

    let previous_dir = session.previous_report(&pool_key).at(Phase::Report)?;
    let previous_report = previous_dir.map(|previous| report::load(&previous.dir));
    let previous = match &previous_report {
        None => Previous::None,
        Some(Ok(report)) => Previous::Read(report),
        Some(Err(_)) => Previous::Unreadable,
    };
    let history = session.history().at(Phase::Report)?;
    let acceptances = session.acceptances().at(Phase::Report)?;
    // One order for both: `read_outputs` reads the records of the pool as `Context.pool` lists it.
    let (pool_dirs, pool_reports): (Vec<Utf8PathBuf>, Vec<Report>) = pool
        .into_iter()
        .map(|(run, report)| (run.dir, report))
        .unzip();
    let outputs = report::read_outputs(&raw, &pool_dirs).at(Phase::Report)?;
    let context = context::of(Known {
        run: &run.id,
        created_at: now,
        root: worktree.root.clone(),
        head_tree: head.to_string(),
        stand: &stand,
        history: &history,
        acceptances: &acceptances,
        previous,
        pool: &pool_reports,
        setup_from: cache::setup_from(&head_plan),
        reused,
    });
    let report = report::make(&raw, &outputs, &context);
    report.write(&run.dir).at(Phase::Report)?;
    session.passed(&run).at(Phase::Report)?;
    Ok(Done::Passed(Box::new(report)))
}

/// After the run, still under the lock: the cache slots on 0 and 1, what this run built gone on
/// a halt (plan, row 88); the kept run on every path (row 64); the tar files always. A failure
/// here turns a passed run into exit 3; after a halt it only adds a line to stderr.
fn settle(
    outcome: Result<Done, Halt>,
    made: &Made,
    sandbox: &impl Sandbox,
    store: &Store,
    err: &mut dyn Write,
) -> Ended {
    let mut settled = Ok(());
    if let (Some(session), Some(run)) = (&made.session, &made.run) {
        settled = match &outcome {
            Ok(_) => cache::keep(session, &made.plans, sandbox, store),
            Err(_) => cache::discard(&made.plans, sandbox, store),
        }
        .and_then(|()| keep_run(session, run, sandbox));
    }
    let settled = settled
        .and_then(|()| Ok(store.clear_trees()?))
        .at(Phase::Report);
    let run_id = made.run.as_ref().map(|run| run.id.clone());
    match (outcome, settled) {
        (Err(halt), settled) => {
            if let Err(also) = settled {
                // A stderr that fails has nowhere left to report it.
                let _ = writeln!(err, "{}", also.error);
            }
            Ended::Halted { halt, run: run_id }
        }
        (Ok(_), Err(halt)) => Ended::Halted { halt, run: run_id },
        (Ok(Done::Passed(report)), Ok(())) => Ended::Passed {
            run: made.run.clone().expect("a passed run has its folder"),
            report,
        },
        (Ok(Done::Failed(failed)), Ok(())) => Ended::Failed {
            run: made.run.clone().expect("a failed run has its folder"),
            failed,
        },
    }
}

/// A run that wrote `kept.json` is the session's kept run; the snapshots of the one before go.
fn keep_run(session: &Session, run: &RunPaths, sandbox: &impl Sandbox) -> Result<(), StandError> {
    if run::kept_ids(&run.dir)?.is_empty() {
        return Ok(());
    }
    if let Some(previous) = session.keep(run)? {
        sweep::drop_snapshots(sandbox, &[], &[previous])?;
    }
    Ok(())
}

/// Passed runs of the same trees and recipe whose report loads, oldest first; a report that does
/// not load is left out with its folder (Gate 1, «Прошлый отчёт в `make`»).
fn pool(session: &Session, key: &PoolKey) -> Result<Vec<(RunPaths, Report)>, StandError> {
    Ok(session
        .pool(key)?
        .into_iter()
        .filter_map(|run| report::load(&run.dir).ok().map(|report| (run, report)))
        .collect())
}

/// The repeats outside the stats this run makes, and those it takes from the last report of its
/// pool (plan, row 89): a result is taken when that report carries it, unless a fault category
/// was unknown there. A repeat the stand does not ask for is in neither.
fn extras(stand: &Stand, last: Option<&Report>) -> (BTreeSet<Extra>, BTreeSet<Extra>) {
    let mut asked = BTreeSet::new();
    if stand
        .services
        .values()
        .any(|service| service.profile.is_some())
    {
        asked.insert(Extra::Coverage);
    }
    if stand.fault.is_some() {
        asked.insert(Extra::Fault);
    }
    if stand.second_pass.is_some() {
        asked.insert(Extra::SecondPass);
    }
    let Some(last) = last else {
        return (asked, BTreeSet::new());
    };
    let carried = |extra: &Extra| match extra {
        Extra::Coverage => last.coverage.is_some(),
        Extra::Fault => last.fault.as_ref().is_some_and(|fault| !fault.unknown()),
        Extra::SecondPass => last.second_pass.is_some(),
    };
    asked.into_iter().partition(|extra| !carried(extra))
}

fn print(ended: Ended, output: Output, env: &mut Env<'_>) -> Exit {
    // A stdout or stderr that fails has nowhere left to report it; the exit code still tells.
    match ended {
        Ended::Passed { run, report } => {
            let line = report.line(&run.dir);
            let _ = match output {
                Output::Json(layout) => next::print_line(env.out, &line, layout),
                Output::Summary => write!(env.out, "{}", report.summary())
                    .and_then(|()| {
                        let path = run.dir.join("report.json");
                        writeln!(env.out, "report {}", slashed(&path))
                    })
                    .and_then(|()| next::print_next(env.out, &line.next)),
            };
            Exit::Passed
        }
        Ended::Failed { run, failed } => {
            let next = inspect_next(&run.id, &failed.kept);
            let _ = writeln!(env.err, "the head failed: {}", one_line(&failed.failure));
            let mut line = Line::failed(failed.failure.clone(), Some(&failed.raw), next);
            line.run = Some(run.id);
            let _ = match output {
                Output::Json(layout) => next::print_line(env.out, &line, layout),
                Output::Summary => write!(env.out, "{}", line.summary())
                    .and_then(|()| next::print_next(env.out, &line.next)),
            };
            Exit::AppFailed
        }
        Ended::Halted { halt, run } => {
            let exit = halt.error.exit();
            let next = halt.error.next();
            let _ = writeln!(env.err, "{}", halt.error);
            if matches!(exit, Exit::Refused | Exit::Broken) {
                let _ = next::print_next(env.err, &next);
            }
            if let Output::Json(layout) = output {
                let cause = match exit {
                    Exit::Cancelled => Cause::Cancelled,
                    _ => Cause::Error(
                        halt.error
                            .to_string()
                            .lines()
                            .collect::<Vec<_>>()
                            .join("; "),
                    ),
                };
                let failure = Failure {
                    phase: halt.phase,
                    service: None,
                    repeat: None,
                    cause,
                    logs: None,
                    probes: Vec::new(),
                };
                let mut line = Line::failed(failure, None, next);
                line.run = run;
                let _ = next::print_line(env.out, &line, layout);
            }
            exit
        }
    }
}

/// Exit 1: the failure snapshot of each volume service of the head, or the recipe's rules when
/// there is none (setup failed, no volume): every failure has a next step (plan, decision 26).
fn inspect_next(run: &str, kept: &[String]) -> Vec<Next> {
    if kept.is_empty() {
        return vec![Next::Guide(Some(Topic::Recipe))];
    }
    kept.iter()
        .map(|service| Next::Inspect {
            run: run.to_owned(),
            service: service.clone(),
        })
        .collect()
}

fn one_line(failure: &Failure) -> String {
    // The phase's token, as the line prints it: `setup`, `workload`.
    let phase = serde_json::to_value(failure.phase).expect("a phase serializes");
    let mut text = format!("{} {}", phase.as_str().unwrap_or_default(), failure.cause);
    if let Some(service) = &failure.service {
        text.push_str(&format!(" in {service}"));
    }
    if let Some(repeat) = failure.repeat {
        text.push_str(&format!(", repeat {repeat}"));
    }
    if let Some(logs) = &failure.logs {
        text.push_str(&format!("; logs {}", slashed(logs)));
    }
    text
}

/// Ctrl+C between the steps `run` does not watch itself.
fn checkpoint(cancel: &Cancel, phase: Phase) -> Result<(), Halt> {
    if cancel.is_cancelled() {
        return Err(StandError::Cancelled).at(phase);
    }
    Ok(())
}

/// A tar that fails halfway is deleted: a partial one could look whole (review of T-08).
fn write_tar(worktree: &Worktree, tree: &Tree, path: &Utf8Path) -> Result<(), StandError> {
    let io = |source| StandError::Io {
        path: path.to_owned(),
        source,
    };
    let written = File::create(path).map_err(io).and_then(|mut file| {
        source::archive(worktree, tree, &mut file)?;
        file.sync_all().map_err(io)
    });
    if written.is_err() {
        // The archive's own error is the one to report.
        let _ = std::fs::remove_file(path);
    }
    written
}

fn demand(stand: &Stand) -> budget::Demand {
    let demand = stand.demand();
    budget::Demand {
        cpus: demand.cpus,
        memory: demand.memory,
    }
}

fn image_refs(stand: &Stand) -> Vec<ImageRef> {
    images(stand).into_iter().map(ImageRef::new).collect()
}

/// Paths print with `/` on every OS (plan, row 45).
fn slashed(path: &Utf8Path) -> String {
    path.as_str().replace('\\', "/")
}
