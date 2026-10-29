//! `run` and `run --dry-run`: the call order of the plan («Команды `stop-slop-stand`», steps
//! 1-8). Every step tags its error with its phase; whatever happens after the run folder exists,
//! the cache slots, the kept run and the tar files are settled under the lock before it goes.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::Write;
use std::time::SystemTime;

use camino::{Utf8Path, Utf8PathBuf};
use report::{Cause, Extra, Failure, Line, Next, Phase, Previous, Report, Topic};
use run::{Cancel, Failed, RunError, Version, Versions};
use sandbox::{ImageRef, Sandbox};
use source::{Before, Resolved, Tree, Worktree};
use spec::{Policy, Stand};
use store::{PoolKey, RunPaths, Session, Slot, Store};

use crate::cache::{self, Planned};
use crate::context::{self, Known};
use crate::error::{At, Halt, StandError};
use crate::{Compare, Env, Exit, Output, images, lock, next, policy, seal, stand_file, sweep};

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
    policy: Policy,
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

pub(crate) fn run(
    compare: Option<&Compare>,
    output: Output,
    sandbox: &impl Sandbox,
    env: &mut Env<'_>,
) -> Exit {
    let ended = match opened(env) {
        Err(halt) => Ended::Halted { halt, run: None },
        Ok(opened) => {
            let span = run::span(&opened.live);
            match lock::wait(env.store, span, env.cancel, &mut *env.err).at(Phase::Lock) {
                Err(halt) => Ended::Halted { halt, run: None },
                Ok(_lock) => {
                    let mut made = Made::default();
                    let outcome = measure(opened, compare, sandbox, env, &mut made);
                    settle(outcome, &made, sandbox, env.store, &mut *env.err)
                }
            }
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
    let checked = opened(env).and_then(|opened| {
        let span = run::span(&opened.live);
        let _lock = lock::wait(env.store, span, env.cancel, &mut *env.err).at(Phase::Lock)?;
        dry_checks(opened, before, sandbox, env)
    });
    let text = match checked {
        Err(halt) => return print(Ended::Halted { halt, run: None }, output, env),
        Ok(text) => text,
    };
    // A stdout that fails has nowhere left to report it; the exit code still tells.
    let _ = match output {
        Output::Json(layout) => next::print_line(env.out, &Line::default(), layout),
        Output::Summary => write!(env.out, "{text}"),
    };
    Exit::Passed
}

fn opened(env: &Env<'_>) -> Result<Opened, Halt> {
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
    let Opened {
        worktree,
        file,
        policy,
        share,
        live,
    } = opened;
    let store = env.store;
    // One time for the session, the seal and the run's record: a seal later than the run that
    // made it would count that run as measured before the seal.
    let now = SystemTime::now();
    let (head, before) = trees(&worktree, compare.map(|compare| &compare.before))?;
    checkpoint(env.cancel, Phase::Source)?;
    // A run with `--before` reads the sealed copy, whose images are the live ones the session
    // names: the seal lets a run through only when every file of the copy is the live one.
    let session = store
        .session(&file, &worktree.root, &images(&live), now)
        .at(Phase::Seal)?;
    let key = session.key().to_owned();
    made.session = Some(session);
    let session = made.session.as_ref().expect("just set");
    let (stand, sealed) = match compare {
        None => (live, None),
        Some(compare) => {
            let reseal = compare.reseal.as_ref();
            let (stand, sealed) =
                seal::check(session, &live, &compare.before, reseal, &policy, now)
                    .at(Phase::Seal)?;
            (stand, Some(sealed))
        }
    };
    checkpoint(env.cancel, Phase::Seal)?;

    sweep::leftovers(sandbox, store, now).at(Phase::Sweep)?;
    checkpoint(env.cancel, Phase::Sweep)?;
    let host = sweep::make_room(sandbox, store, &key, env.host).at(Phase::Budget)?;
    budget::check(demand(&stand), &host, share).at(Phase::Budget)?;
    let pulled = sandbox.pull(&image_refs(&stand)).at(Phase::Pull)?;
    checkpoint(env.cancel, Phase::Pull)?;

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
    let run = session.new_run(now, &pool_key).at(Phase::Source)?;
    made.run = Some(run.clone());

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

/// The steps of `run` up to the pull, with nothing pulled, sealed or built: SDK 0.7.8 pulls a
/// missing image through a VM (plan T-11, orchestrator), so the images come from `cached`, the
/// adapter's read of its local cache. The trees and the seal are checked as `run` checks them, so
/// a refusal of `run` is a refusal here too. A setup can come from the cache only when every image
/// is in the home: the key holds each digest.
fn dry_checks(
    opened: Opened,
    before: Option<&Before>,
    sandbox: &impl Sandbox,
    env: &mut Env<'_>,
) -> Result<String, Halt> {
    let Opened {
        worktree,
        file,
        policy,
        share,
        live,
    } = opened;
    let store = env.store;
    let now = SystemTime::now();
    let (head, before) = trees(&worktree, before)?;
    checkpoint(env.cancel, Phase::Source)?;
    let session = store
        .session(&file, &worktree.root, &images(&live), now)
        .at(Phase::Seal)?;
    let (stand, sealed) = match &before {
        None => (live, None),
        Some(resolved) => {
            seal::compare(&session, live, &resolved.intent, &policy).at(Phase::Seal)?
        }
    };
    checkpoint(env.cancel, Phase::Seal)?;
    sweep::leftovers(sandbox, store, now).at(Phase::Sweep)?;
    checkpoint(env.cancel, Phase::Sweep)?;
    let host = sweep::make_room(sandbox, store, session.key(), env.host).at(Phase::Budget)?;
    let admitted = budget::check(demand(&stand), &host, share).at(Phase::Budget)?;
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
        "dry run: {} ({recipe}), recipe {}\nbudget: {} vCPUs and {} MiB fit the host\n",
        slashed(&file),
        stand.recipe.digest,
        admitted.cpus,
        admitted.memory >> 20,
    );
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
