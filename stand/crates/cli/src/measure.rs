//! `run` and `run --dry-run`: the call order of the plan («Команды `stop-slop-stand`», steps
//! 1-8). Every step tags its error with its phase; whatever happens after the run folder exists,
//! the cache slots, the kept run and the tar files are settled under the lock before it goes.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::Write;
use std::time::SystemTime;

use budget::Resource;
use camino::{Utf8Path, Utf8PathBuf};
use report::{Cause, Extra, Failure, Line, Next, Phase, Previous, Report, Topic};
use run::{Cancel, Failed, RunError, Version, Versions};
use sandbox::{ImageRef, Sandbox};
use source::{Before, Resolved, Tree, Worktree};
use spec::Stand;
use store::{PoolKey, RunLock, RunPaths, Session, Slot, Store};

use crate::cache::{self, Planned};
use crate::context::{self, Known};
use crate::error::{At, Halt, StandError};
use crate::{
    Compare, Env, Exit, Opened, Output, error_cause, images, next, refusal_line, refused, seal,
    slashed, sweep,
};

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

pub(crate) fn run(
    compare: Option<&Compare>,
    output: Output,
    sandbox: &impl Sandbox,
    env: &mut Env<'_>,
) -> Exit {
    let ended = match locked(env) {
        Err(halt) => Ended::Halted { halt, run: None },
        Ok((_lock, opened)) => {
            let mut made = Made::default();
            let outcome = measure(opened, compare, sandbox, env, &mut made);
            settle(outcome, made, sandbox, env.store, &mut *env.err)
        }
    };
    print(ended, output, env)
}

pub(crate) fn dry(
    before: Option<&Before>,
    output: Output,
    sandbox: &impl Sandbox,
    env: &mut Env<'_>,
) -> Exit {
    let checked = locked(env).and_then(|(_lock, opened)| dry_checks(opened, before, sandbox, env));
    let text = match checked {
        Err(halt) => return print(Ended::Halted { halt, run: None }, output, env),
        Ok(text) => text,
    };
    let _ = match output {
        Output::Json(layout) => next::print_line(env.out, &Line::default(), layout),
        Output::Summary => write!(env.out, "{text}"),
    };
    Exit::Passed
}

/// Steps 1 and 2: what is read before the lock, then the lock.
fn locked(env: &mut Env<'_>) -> Result<(RunLock, Opened), Halt> {
    Opened::read(env)?.locked(env)
}

/// Steps 4 and 5 of `run`: the sweep, then the budget, and room on the disk only when the disk is
/// all it refuses: a run that does not fit the CPUs or the memory keeps what it would drop.
fn admit(
    stand: &Stand,
    share: budget::Share,
    session: &Session,
    sandbox: &impl Sandbox,
    env: &Env<'_>,
    now: SystemTime,
) -> Result<budget::Admission, Halt> {
    sweep::leftovers(sandbox, env.store, now).at(Phase::Sweep)?;
    checkpoint(env.cancel, Phase::Sweep)?;
    let demand = demand(stand);
    let host = (env.host)(&env.store.paths().root).at(Phase::Budget)?;
    match budget::check(demand, &host, share) {
        Err(refusal) if refusal.resource == Resource::Disk => {
            let host = sweep::make_room(sandbox, env.store, session.key(), env.host, host)
                .at(Phase::Budget)?;
            budget::check(demand, &host, share).at(Phase::Budget)
        }
        checked => checked.at(Phase::Budget),
    }
}

/// `admit` for `--dry-run`, which drops nothing: what the sweep and a low disk would take, then
/// the budget. A disk that only those drops can free passes, with lines that say what a run drops
/// first; whether that makes room enough only the run sees.
fn dry_admit(
    stand: &Stand,
    share: budget::Share,
    session: &Session,
    sandbox: &impl Sandbox,
    env: &Env<'_>,
    now: SystemTime,
) -> Result<(budget::Admission, Vec<String>), Halt> {
    let mut lines = sweep::would_sweep(sandbox, env.store, now).at(Phase::Sweep)?;
    checkpoint(env.cancel, Phase::Sweep)?;
    let demand = demand(stand);
    let host = (env.host)(&env.store.paths().root).at(Phase::Budget)?;
    let refusal = match budget::check(demand, &host, share) {
        Ok(admitted) => return Ok((admitted, lines)),
        Err(refusal) => refusal,
    };
    let room = match refusal.resource {
        Resource::Disk => {
            sweep::would_make_room(env.store, session.key(), now).at(Phase::Budget)?
        }
        Resource::Cpu | Resource::Memory => Vec::new(),
    };
    if room.is_empty() {
        return Err(StandError::Budget(refusal)).at(Phase::Budget);
    }
    lines.push(format!(
        "{refusal}: a run drops in turn until there is room"
    ));
    lines.extend(room);
    // `check` takes the disk last: the CPUs and the memory fit.
    let admitted = budget::Admission {
        cpus: demand.cpus,
        memory: demand.memory,
    };
    Ok((admitted, lines))
}

/// The first step under the lock: a missing snapshot, an unknown ref or a "before" equal to the
/// head refuses before the seal, the sweep and the pull.
fn trees(worktree: &Worktree, before: Option<&Before>) -> Result<(Tree, Option<Resolved>), Halt> {
    let head = source::snapshot(worktree).at(Phase::Source)?;
    let Some(before) = before else {
        return Ok((head, None));
    };
    let resolved = source::before(worktree, before).at(Phase::Source)?;
    if resolved.tree == head {
        return Err(StandError::NothingToCompare).at(Phase::Source);
    }
    Ok((head, Some(resolved)))
}

/// Steps 3-8 under the lock, up to the written report.
fn measure(
    opened: Opened,
    compare: Option<&Compare>,
    sandbox: &impl Sandbox,
    env: &mut Env<'_>,
    made: &mut Made,
) -> Result<Done, Halt> {
    let store = env.store;
    // One time for the session, the seal and the run's record: a seal later than the run that
    // made it would count that run as measured before the seal.
    let now = SystemTime::now();
    let (head, before) = trees(&opened.worktree, compare.map(|compare| &compare.before))?;
    checkpoint(env.cancel, Phase::Source)?;
    // A run with `--before` reads the sealed copy, whose images are the live ones the session
    // names: the seal lets a run through only when every file of the copy is the live one.
    let session = opened.open_session(store, now).at(Phase::Seal)?;
    let session = &*made.session.insert(session);
    let Opened {
        worktree,
        policy,
        share,
        live,
        ..
    } = opened;
    let reseal = compare.and_then(|compare| compare.reseal.as_ref());
    let (stand, sealed) = match compare {
        None => (live, None),
        Some(compare) => {
            seal::compare(session, live, &compare.before, reseal, &policy).at(Phase::Seal)?
        }
    };
    checkpoint(env.cancel, Phase::Seal)?;

    admit(&stand, share, session, sandbox, env, now)?;
    let pulled = sandbox.pull(&image_refs(&stand)).at(Phase::Pull)?;
    checkpoint(env.cancel, Phase::Pull)?;
    let (stand, sealed) = match (compare, sealed) {
        (Some(_), None) => {
            let (stand, sealed) =
                seal::seal(session, &stand, reseal, &policy, now).at(Phase::Seal)?;
            (stand, Some(sealed))
        }
        (_, sealed) => (stand, sealed),
    };

    let tars = store.trees().at(Phase::Source)?;
    write_tar(&worktree, &head, &tars.head).at(Phase::Source)?;
    let changed = match &before {
        None => None,
        Some(resolved) => {
            write_tar(&worktree, &resolved.tree, &tars.before).at(Phase::Source)?;
            Some(source::changed(&worktree, &resolved.tree, &head).at(Phase::Source)?)
        }
    };
    checkpoint(env.cancel, Phase::Source)?;

    let pool_key = PoolKey {
        head_tree: head.to_string(),
        before_tree: before.as_ref().map(|resolved| resolved.tree.to_string()),
        digest: stand.recipe.digest.clone(),
    };
    let run = &*made
        .run
        .insert(session.new_run(now, &pool_key).at(Phase::Source)?);

    let left = sandbox.leftovers().at(Phase::Setup)?;
    let head_key = cache::key(&stand, &head, &pulled);
    let head_plan =
        cache::plan(session, Slot::Head, head_key, &left, &run.id, sandbox).at(Phase::Setup)?;
    made.plans.push(head_plan.clone());
    let before_plan = match &before {
        None => None,
        Some(resolved) => {
            let key = cache::key(&stand, &resolved.tree, &pulled);
            let planned = cache::plan(session, Slot::Before, key, &left, &run.id, sandbox)
                .at(Phase::Setup)?;
            made.plans.push(planned.clone());
            Some(planned)
        }
    };

    let pool = pool(session, &pool_key).at(Phase::Report)?;
    let (extras, reused) = extras(&stand, pool.last().map(|(_, report)| report));
    let versions = Versions {
        head: Version {
            tar: tars.head.clone(),
            setup: head_plan.setup(),
        },
        before: before_plan.as_ref().map(|planned| Version {
            tar: tars.before.clone(),
            setup: planned.setup(),
        }),
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
        before: before.as_ref().zip(changed),
        stand: &stand,
        seal: sealed.as_ref().map(|sealed| (sealed, policy)),
        history: &history,
        acceptances: &acceptances,
        previous,
        pool: &pool_reports,
        setup_from: cache::setup_from(&head_plan, before_plan.as_ref()),
        reused,
    });
    let report = report::make(&raw, &outputs, &context);
    report.write(&run.dir).at(Phase::Report)?;
    session.passed(run).at(Phase::Report)?;
    Ok(Done::Passed(Box::new(report)))
}

/// After the run, still under the lock: the cache slots on 0 and 1, what this run built gone on
/// a halt (plan, row 88); the kept run on every path (row 64); the tar files always. A failure
/// here turns a passed run into exit 3; after a halt it only adds a line to stderr.
fn settle(
    outcome: Result<Done, Halt>,
    made: Made,
    sandbox: &impl Sandbox,
    store: &Store,
    err: &mut dyn Write,
) -> Ended {
    let Made {
        session,
        run,
        plans,
    } = made;
    // Each step on its own, the first error kept: a cache that fails to settle costs neither the
    // kept run nor the tar files.
    let mut settled = Ok(());
    if let (Some(session), Some(run)) = (&session, &run) {
        let cache = match &outcome {
            Ok(_) => cache::keep(session, &plans, sandbox, store),
            Err(_) => cache::discard(&plans, sandbox, store),
        };
        let kept = keep_run(session, run, sandbox);
        settled = cache.and(kept);
    }
    let trees = store.clear_trees().map_err(StandError::from);
    let settled = settled.and(trees).at(Phase::Report);
    match (outcome, settled, run) {
        (Err(halt), settled, run) => {
            if let Err(also) = settled {
                let _ = writeln!(err, "{}", also.error);
            }
            Ended::Halted {
                halt,
                run: run.map(|run| run.id),
            }
        }
        (Ok(done), Err(halt), run) => {
            // The halt is the outcome now; the failure of the head still shows.
            if let Done::Failed(failed) = &done {
                let _ = writeln!(err, "the head failed: {}", one_line(&failed.failure));
            }
            Ended::Halted {
                halt,
                run: run.map(|run| run.id),
            }
        }
        (Ok(done), Ok(()), run) => {
            let run = run.expect("a run that ended has its folder");
            match done {
                Done::Passed(report) => Ended::Passed { run, report },
                Done::Failed(failed) => Ended::Failed { run, failed },
            }
        }
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
    asked.into_iter().partition(|extra| !last.carries(*extra))
}

/// The steps of `run` up to the pull, with nothing pulled, sealed, built or dropped: SDK 0.7.8
/// pulls a missing image through a VM (plan T-11, orchestrator), so the images come from `cached`,
/// the adapter's read of its local cache. The trees and the seal are checked as `run` checks them,
/// so a refusal of `run` is a refusal here too. A setup can come from the cache only when every
/// image is in the home: the key holds each digest.
fn dry_checks(
    opened: Opened,
    before: Option<&Before>,
    sandbox: &impl Sandbox,
    env: &mut Env<'_>,
) -> Result<String, Halt> {
    let now = SystemTime::now();
    let (head, before) = trees(&opened.worktree, before)?;
    checkpoint(env.cancel, Phase::Source)?;
    let session = opened.open_session(env.store, now).at(Phase::Seal)?;
    let Opened {
        file,
        policy,
        share,
        live,
        ..
    } = opened;
    let (stand, sealed) = match &before {
        None => (live, None),
        Some(resolved) => {
            seal::compare(&session, live, &resolved.intent, None, &policy).at(Phase::Seal)?
        }
    };
    checkpoint(env.cancel, Phase::Seal)?;
    let (admitted, dropped) = dry_admit(&stand, share, &session, sandbox, env, now)?;
    let cached = sandbox.cached(&image_refs(&stand)).at(Phase::Pull)?;
    checkpoint(env.cancel, Phase::Pull)?;

    let recipe = match &sealed {
        Some(sealed) => format!(
            "sealed copy of {}",
            humantime::format_rfc3339_seconds(sealed.sealed_at)
        ),
        None => "live folder".to_owned(),
    };
    let mut text = format!(
        "dry run: {} ({recipe}), recipe {}\n",
        slashed(&file),
        stand.recipe.digest,
    );
    for line in dropped {
        text.push_str(&format!("{line}\n"));
    }
    text.push_str(&format!(
        "budget: {} vCPUs and {} MiB fit the host\n",
        admitted.cpus,
        admitted.memory >> 20,
    ));
    for (image, digest) in &cached.digests {
        match digest {
            Some(digest) => text.push_str(&format!("image {image} cached {digest}\n")),
            None => text.push_str(&format!("image {image} to download\n")),
        }
    }
    let pulled = cache::known(&cached);
    let left = sandbox.leftovers().at(Phase::Setup)?;
    let mut sides = vec![("head", &head)];
    sides.extend(before.as_ref().map(|resolved| ("before", &resolved.tree)));
    for (side, tree) in sides {
        let built_by = match &pulled {
            Some(pulled) => {
                let key = cache::key(&stand, tree, pulled);
                cache::would_hit(&session, &key, &left).at(Phase::Setup)?
            }
            None => None,
        };
        match built_by {
            Some(run) => text.push_str(&format!("setup {side} cached {run}\n")),
            None => text.push_str(&format!("setup {side} built\n")),
        }
    }
    Ok(text)
}

fn print(ended: Ended, output: Output, env: &mut Env<'_>) -> Exit {
    match ended {
        Ended::Passed { run, report } => {
            let line = report.line(&run.dir);
            let _ = match output {
                Output::Json(layout) => next::print_line(env.out, &line, layout),
                Output::Summary => write!(env.out, "{}", report.summary())
                    .and_then(|()| match &line.report {
                        Some(path) => writeln!(env.out, "report {}", slashed(path)),
                        None => Ok(()),
                    })
                    .and_then(|()| next::print_next(env.out, &line.next)),
            };
            Exit::Passed
        }
        Ended::Failed { run, failed } => {
            let Failed { failure, raw, kept } = *failed;
            let next = inspect_next(&run.id, &kept);
            let _ = writeln!(env.err, "the head failed: {}", one_line(&failure));
            let mut line = Line::failed(failure, Some(&raw), next);
            line.run = Some(run.id);
            let _ = match output {
                Output::Json(layout) => next::print_line(env.out, &line, layout),
                Output::Summary => write!(env.out, "{}", line.summary())
                    .and_then(|()| next::print_next(env.out, &line.next)),
            };
            Exit::AppFailed
        }
        Ended::Halted { halt, run } => {
            let exit = refused(&halt.error, env.err);
            if let Output::Json(layout) = output {
                let cause = match exit {
                    Exit::Cancelled => Cause::Cancelled,
                    _ => error_cause(&halt.error.to_string()),
                };
                let mut line = refusal_line(halt.phase, cause, halt.error.next());
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

/// A tar that fails halfway is deleted: a partial one could look whole (review of T-08). It lives
/// for this run only, so it is not synced to the disk.
fn write_tar(worktree: &Worktree, tree: &Tree, path: &Utf8Path) -> Result<(), StandError> {
    let written = File::create(path)
        .map_err(StandError::io(path))
        .and_then(|mut file| Ok(source::archive(worktree, tree, &mut file)?));
    if written.is_err() {
        // The archive's own error is the one to report.
        let _ = std::fs::remove_file(path);
    }
    written
}

pub(crate) fn demand(stand: &Stand) -> budget::Demand {
    let demand = stand.demand();
    budget::Demand {
        cpus: demand.cpus,
        memory: demand.memory,
    }
}

fn image_refs(stand: &Stand) -> Vec<ImageRef> {
    images(stand).into_iter().map(ImageRef::new).collect()
}
