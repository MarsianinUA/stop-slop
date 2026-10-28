//! The lane of each version (architecture plan, row 10) and the steps of one repeat. Each version
//! is set up or taken from its cache entry, "before" first, then the measured repeats go in turns,
//! "before" first, then the repeats outside the stats that the set of extras names (row 89). A
//! failure of "before" stops its lane only; a failure of the head ends the run. Only one phase has
//! VMs at a time, so no two live VMs share a service and a replica: the adapter keeps one host
//! port per (service, replica, port) (plan T-11, decision 7).

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use profile::{Called, Coverage, ProfileMode};
use report::{
    BeforeOutcome, Cause, CoverageOutcome, Extra, Failure, FaultOutcome, FaultRepeat,
    InstanceOutput, Pass, PassPair, Phase, ProbeRan, ProbeRule, Raw, RepeatOutput, Role,
    SecondPassOutcome, Setup, SetupSide,
};
use sandbox::{Fault, Metrics, Sandbox, SnapshotId};
use spec::{Condition, ServiceKind, ServiceName, Stand, Tap};

use crate::keep::{self, Kept};
use crate::launch::{self, Frame, Seeds};
use crate::ready::{Fleet, Halt};
use crate::{
    Cancel, Failed, RunError, SetupCache, Side, StandFault, Version, Versions, cache, drain, fault,
    probes, span,
};

/// The run stops in `Phase`: a breakage of the stand, or `Cancel`.
enum Stop {
    Fault(Phase, StandFault),
    Cancelled(Phase),
}

impl From<Stop> for RunError {
    fn from(stop: Stop) -> RunError {
        match stop {
            Stop::Fault(phase, fault) => RunError::Stand { phase, fault },
            Stop::Cancelled(phase) => RunError::Cancelled { phase },
        }
    }
}

/// A version's lane once its setup is over.
enum Lane {
    /// The repeats start from `seeds`; `outputs` are its measured repeats so far.
    Ready {
        seeds: Seeds,
        outputs: Vec<RepeatOutput>,
    },
    /// What stopped it, and the repeats it ran, the failed one included.
    Failed {
        failure: Failure,
        outputs: Vec<RepeatOutput>,
    },
}

#[derive(Clone, Copy)]
enum Kind {
    Measured(u32),
    /// The coverage repeat of the head, out of every measure (architecture plan, row 38).
    Coverage,
    /// A repeat under the fault of the stand (row 73), out of every measure.
    Fault(u32),
}

struct Repeated {
    output: RepeatOutput,
    failure: Option<Failure>,
    /// By profiled service, its replicas as one: only the coverage repeat that passed reads them.
    calls: BTreeMap<String, Coverage>,
    /// Of a fault repeat.
    ends: Option<fault::Ends>,
}

struct Ctx<'r, S> {
    stand: &'r Stand,
    sandbox: &'r S,
    extras: &'r BTreeSet<Extra>,
    run_dir: &'r Utf8Path,
    cancel: &'r Cancel,
    span: Duration,
    /// `run_dir/preload`, with `preload.cjs` when a service of the repeats is profiled.
    preload: Utf8PathBuf,
    /// Setup snapshots no cache entry holds: they go at the end.
    loose: Vec<SnapshotId>,
    /// The end-of-run snapshots so far, for `kept.json`.
    kept: Vec<Kept>,
}

/// `Raw`, `Extra` and their types are `report`'s (plan, rows 85, 89); `run` fills them and keeps
/// no copy. An extra the `Stand` does not ask for does not run even when `extras` lists it.
pub fn run(
    stand: &Stand,
    sandbox: &impl Sandbox,
    versions: &Versions,
    extras: &BTreeSet<Extra>,
    run_dir: &Utf8Path,
    cancel: &Cancel,
) -> Result<Raw, RunError> {
    // The preload goes before the first setup, in its phase.
    let ctx = Ctx::new(stand, sandbox, extras, run_dir, cancel);
    let mut ctx = ctx.map_err(|fault| RunError::Stand {
        phase: Phase::Setup,
        fault,
    })?;
    let result = lanes(&mut ctx, versions);
    // On every path; after another error its own error is dropped, the first one ends the run.
    let end = ctx.end();
    let raw = match result {
        Err(RunError::App(mut failed)) => {
            let head = ctx.kept.iter().filter(|kept| kept.side == Side::Head);
            failed.kept = head.map(|kept| kept.service.clone()).collect();
            return Err(RunError::App(failed));
        }
        result => result?,
    };
    end.map_err(|fault| RunError::Stand {
        phase: Phase::Workload,
        fault,
    })?;
    Ok(raw)
}

fn lanes<S: Sandbox>(ctx: &mut Ctx<S>, versions: &Versions) -> Result<Raw, RunError> {
    let before = match &versions.before {
        Some(version) => Some(set_up(ctx, Side::Before, version)?),
        None => None,
    };
    let (head_setup, mut head) = set_up(ctx, Side::Head, &versions.head)?;
    let (before_setup, mut before) = before.unzip();
    for repeat in 1..=ctx.stand.repeat.get() {
        if let Lane::Failed { .. } = head {
            break;
        }
        if let (Some(version), Some(lane)) = (&versions.before, before.as_mut()) {
            measure(ctx, Side::Before, version, lane, repeat)?;
        }
        measure(ctx, Side::Head, &versions.head, &mut head, repeat)?;
    }
    let outside = match &head {
        Lane::Ready { seeds, .. } => {
            let head = Passed {
                version: &versions.head,
                seeds,
            };
            let before = match (&versions.before, &before) {
                (Some(version), Some(Lane::Ready { seeds, .. })) => Some(Passed { version, seeds }),
                _ => None,
            };
            outside(ctx, head, before)?
        }
        Lane::Failed { .. } => Outside::default(),
    };
    let (outputs, failure) = match head {
        Lane::Ready { outputs, .. } => (outputs, None),
        Lane::Failed { failure, outputs } => (outputs, Some(failure)),
    };
    let raw = raw(
        ctx.stand,
        head_setup,
        outputs,
        before_setup,
        before,
        outside,
    );
    match failure {
        None => Ok(raw),
        Some(failure) => Err(RunError::App(Box::new(Failed {
            failure,
            raw,
            // Once `kept.json` is written.
            kept: Vec::new(),
        }))),
    }
}

fn raw(
    stand: &Stand,
    head_setup: SetupSide,
    head: Vec<RepeatOutput>,
    before_setup: Option<SetupSide>,
    before: Option<Lane>,
    outside: Outside,
) -> Raw {
    let harness: BTreeSet<String> = stand
        .services
        .values()
        .flat_map(|service| &service.mounts)
        .filter_map(|mount| match mount {
            spec::Mount::Harness { target, .. } => Some(target.clone()),
            _ => None,
        })
        .collect();
    let probes = stand.probes.iter().map(|(name, probe)| {
        let rule = ProbeRule {
            key: probe.key.as_ref().map(|key| key.as_str().to_owned()),
            ignore: probe
                .ignore
                .iter()
                .map(|path| path.as_str().to_owned())
                .collect(),
        };
        (probes::file_name(name), rule)
    });
    Raw {
        digest: stand.recipe.digest.clone(),
        repeat: stand.repeat.get(),
        setup_timeout: stand.setup_timeout.get(),
        timeout: stand.timeout.get(),
        harness: harness.into_iter().collect(),
        probes: probes.collect(),
        setup: Setup {
            head: head_setup,
            before: before_setup,
        },
        head,
        before: before.map(|lane| match lane {
            Lane::Ready { outputs, .. } => BeforeOutcome::Passed(outputs),
            Lane::Failed { failure, .. } => BeforeOutcome::Failed(failure),
        }),
        coverage: outside.coverage,
        fault: outside.fault,
        second_pass: outside.second_pass,
    }
}

#[derive(Default)]
struct Outside {
    coverage: Option<CoverageOutcome>,
    fault: Option<FaultOutcome>,
    second_pass: Option<SecondPassOutcome>,
}

/// A version whose lane passed its measured repeats, and what they start from.
#[derive(Clone, Copy)]
struct Passed<'a> {
    version: &'a Version,
    seeds: &'a Seeds,
}

/// After the measured repeats of a head that passed them, each repeat outside the stats that
/// `extras` names and the stand asks for: the coverage repeat of the head, the fault repeats, the
/// second pass of each version, "before" first. `before` takes part only when its lane passed too.
fn outside<S: Sandbox>(
    ctx: &mut Ctx<S>,
    head: Passed,
    before: Option<Passed>,
) -> Result<Outside, Stop> {
    let stand = ctx.stand;
    let coverage = if ctx.extras.contains(&Extra::Coverage) && launch::any_profiled(stand) {
        Some(coverage(ctx, head)?)
    } else {
        None
    };
    let fault = match &stand.fault {
        Some(fault) if ctx.extras.contains(&Extra::Fault) => {
            Some(faults(ctx, fault, head, before)?)
        }
        _ => None,
    };
    let second_pass = match &stand.second_pass {
        Some(pass) if ctx.extras.contains(&Extra::SecondPass) => {
            let before = before.map(|before| second_pass(ctx, Side::Before, before));
            let before = before.transpose()?;
            let head = second_pass(ctx, Side::Head, head)?;
            Some(SecondPassOutcome {
                unchanged: pass
                    .unchanged
                    .iter()
                    .map(|file| file.file.clone())
                    .collect(),
                head,
                before,
            })
        }
        _ => None,
    };
    Ok(Outside {
        coverage,
        fault,
        second_pass,
    })
}

/// From the cache entry on a hit; else setup as without a cache, and its entry once it passed.
fn set_up<S: Sandbox>(
    ctx: &mut Ctx<S>,
    side: Side,
    version: &Version,
) -> Result<(SetupSide, Lane), Stop> {
    if ctx.cancel.is_cancelled() {
        return Err(Stop::Cancelled(Phase::Setup));
    }
    let fault = |fault| Stop::Fault(Phase::Setup, fault);
    let dir = ctx.run_dir.join(format!("setup-{side}"));
    let out = dir.join("out");
    let outputs = Vec::new();
    let entry = match &version.setup {
        SetupCache::Hit(entry) => {
            let (seeds, setup) = cache::hit(entry, &out).map_err(fault)?;
            return Ok((setup, Lane::Ready { seeds, outputs }));
        }
        SetupCache::Build(entry) => entry,
    };
    fs::create_dir_all(&out)
        .map_err(StandFault::io(&out))
        .map_err(fault)?;
    let (setup, built) = build(ctx, version, &dir, &out)?;
    let lane = match built {
        Ok(seeds) => {
            cache::write(entry, &seeds, &setup).map_err(fault)?;
            // The entry holds them now; `cli` drops them with the entry.
            ctx.loose
                .retain(|loose| !seeds.snapshots().any(|own| own == loose));
            Lane::Ready { seeds, outputs }
        }
        Err(failure) => Lane::Failed { failure, outputs },
    };
    Ok((setup, lane))
}

/// Setup as without a cache (architecture plan, «Тома, снимки и деревья», step 2).
fn build<S: Sandbox>(
    ctx: &mut Ctx<S>,
    version: &Version,
    dir: &Utf8Path,
    out: &Utf8Path,
) -> Result<(SetupSide, Result<Seeds, Failure>), Stop> {
    let mut fleet = Fleet::default();
    let mut seeds = Seeds::default();
    let deadline = ctx.sandbox.now() + ctx.stand.setup_timeout.get();
    let side = |probes, sizes| SetupSide {
        sizes,
        probes,
        out_dir: out.to_owned(),
    };
    let halt = match start_setup(ctx, &mut fleet, &mut seeds, version, out, deadline) {
        Ok(()) => {
            let ended = finish_setup(ctx, &mut fleet, &mut seeds, dir, out);
            let (probes, sizes) = ended.map_err(|fault| {
                torn(
                    ctx.sandbox,
                    &mut fleet,
                    dir,
                    Stop::Fault(Phase::Setup, fault),
                )
            })?;
            return Ok((side(probes, sizes), Ok(seeds)));
        }
        Err(halt) => halt,
    };
    let (service, cause) = fell(&fleet, halt, Cause::SetupTimeout, Phase::Setup)
        .map_err(|stop| torn(ctx.sandbox, &mut fleet, dir, stop))?;
    let probes = fail_setup(ctx, &mut fleet, &seeds, dir, out).map_err(|fault| {
        torn(
            ctx.sandbox,
            &mut fleet,
            dir,
            Stop::Fault(Phase::Setup, fault),
        )
    })?;
    let failure = failure((service, cause), Phase::Setup, None, dir, &probes);
    Ok((side(probes, BTreeMap::new()), Err(failure)))
}

/// The services of setup by levels, each after what it waits for; the version snapshot right after
/// the prepare service exited 0, before anything starts from it. Every setup service exits 0
/// before `deadline`.
fn start_setup<S: Sandbox>(
    ctx: &mut Ctx<S>,
    fleet: &mut Fleet,
    seeds: &mut Seeds,
    version: &Version,
    out: &Utf8Path,
    deadline: Instant,
) -> Result<(), Halt> {
    let stand = ctx.stand;
    let in_setup = stand.in_setup();
    for level in launch::levels(stand, |name, _| in_setup.contains(name)) {
        for name in &level {
            wait_for(ctx, fleet, name, deadline)?;
        }
        version_snapshot(ctx, fleet, seeds)?;
        let frame = Frame {
            out,
            second: None,
            tar: &version.tar,
            seeds,
            mode: None,
            preload: &ctx.preload,
            span: ctx.span,
        };
        fleet.start(ctx.sandbox, launch::starts(stand, &level, &frame))?;
    }
    for index in 0..fleet.members.len() {
        if fleet.members[index].kind == ServiceKind::Setup {
            fleet.completed(ctx.sandbox, index, deadline, ctx.cancel)?;
        }
    }
    // A database that ended with the last setup service would go into the snapshots crashed.
    fleet.settled(ctx.sandbox)?;
    version_snapshot(ctx, fleet, seeds)
}

fn version_snapshot<S: Sandbox>(
    ctx: &mut Ctx<S>,
    fleet: &Fleet,
    seeds: &mut Seeds,
) -> Result<(), Halt> {
    let Some(prepare) = &ctx.stand.prepare else {
        return Ok(());
    };
    let Some(index) = fleet.find(prepare.as_str(), 1) else {
        return Ok(());
    };
    if seeds.version.is_some() || fleet.members[index].exit.is_none() {
        return Ok(());
    }
    let snapshot = ctx.sandbox.snapshot(&fleet.members[index].id)?;
    ctx.loose.push(snapshot.clone());
    seeds.version = Some(snapshot);
    Ok(())
}

/// What `name` waits for, as Compose has it. In a repeat the setup services are not in `fleet`:
/// their condition holds since setup.
fn wait_for<S: Sandbox>(
    ctx: &Ctx<S>,
    fleet: &mut Fleet,
    name: &ServiceName,
    deadline: Instant,
) -> Result<(), Halt> {
    for (dependency, condition) in &ctx.stand.services[name].depends_on {
        match condition {
            Condition::Started => {}
            Condition::Healthy => {
                for index in fleet.replicas(dependency.as_str()) {
                    fleet.healthy(ctx.sandbox, index, deadline, ctx.cancel)?;
                }
            }
            Condition::CompletedSuccessfully => {
                if let Some(index) = fleet.find(dependency.as_str(), 1) {
                    fleet.completed(ctx.sandbox, index, deadline, ctx.cancel)?;
                }
            }
        }
    }
    Ok(())
}

/// After every setup service exited 0: the cut, the `after-setup` probes, the clean stop, the
/// sizes and the snapshot of each VM with a volume, the drain. `metrics.json` of setup shows what
/// the prepare service took of its limits; no measure reads it.
fn finish_setup<S: Sandbox>(
    ctx: &mut Ctx<S>,
    fleet: &mut Fleet,
    seeds: &mut Seeds,
    dir: &Utf8Path,
    out: &Utf8Path,
) -> Result<(Vec<ProbeRan>, BTreeMap<String, u64>), StandFault> {
    let (stand, sandbox) = (ctx.stand, ctx.sandbox);
    let cut = drain::cut(sandbox, fleet).map_err(StandFault::Sandbox)?;
    let probes =
        probes::run(sandbox, stand, fleet, Tap::AfterSetup, out).map_err(StandFault::Sandbox)?;
    let stopped = drain::stop(sandbox, fleet).map_err(StandFault::Sandbox)?;
    let mut sizes = BTreeMap::new();
    for member in fleet
        .members
        .iter()
        .filter(|member| drain::has_volume(member))
    {
        let (own, snapshot) = drain::snapshot(sandbox, &member.id).map_err(StandFault::Sandbox)?;
        ctx.loose.push(snapshot.clone());
        sizes.extend(own);
        seeds
            .volumes
            .insert(member.launch.service.clone(), snapshot);
    }
    drain::write_metrics(dir, fleet, &cut, &stopped)?;
    drain::drain(sandbox, fleet, dir)?;
    Ok((probes, sizes))
}

/// A failed setup ends as a failed repeat does; the snapshots it took go at once.
fn fail_setup<S: Sandbox>(
    ctx: &mut Ctx<S>,
    fleet: &mut Fleet,
    seeds: &Seeds,
    dir: &Utf8Path,
    out: &Utf8Path,
) -> Result<Vec<ProbeRan>, StandFault> {
    let ended = end_phase(ctx, fleet, Some(Tap::OnFailure), None, dir, out)?;
    for snapshot in seeds.snapshots() {
        ctx.sandbox
            .drop_snapshot(snapshot)
            .map_err(StandFault::Sandbox)?;
        ctx.loose.retain(|loose| loose != snapshot);
    }
    Ok(ended.probes)
}

fn measure<S: Sandbox>(
    ctx: &mut Ctx<S>,
    side: Side,
    version: &Version,
    lane: &mut Lane,
    repeat: u32,
) -> Result<(), Stop> {
    let Lane::Ready { seeds, outputs } = lane else {
        return Ok(());
    };
    let repeated = run_repeat(ctx, side, version, seeds, Kind::Measured(repeat))?;
    outputs.push(repeated.output);
    if let Some(failure) = repeated.failure {
        let outputs = std::mem::take(outputs);
        *lane = Lane::Failed { failure, outputs };
    }
    Ok(())
}

/// Two per version in turns, "before" first (architecture plan, «Тома, снимки и деревья», step 5):
/// how each ended is a category, never a failure of the run.
fn faults<S: Sandbox>(
    ctx: &mut Ctx<S>,
    fault: &spec::Fault,
    head: Passed,
    before: Option<Passed>,
) -> Result<FaultOutcome, Stop> {
    let (mut heads, mut befores) = (Vec::new(), Vec::new());
    let ended = |repeat, repeated: Repeated| {
        let ends = repeated.ends.expect("a fault repeat notes how it ended");
        FaultRepeat {
            repeat,
            workload: ends.workload,
            exited: ends.exited,
            failure: repeated.failure,
            instances: repeated.output.instances,
        }
    };
    for n in 1..=fault::REPEATS {
        if let Some(Passed { version, seeds }) = before {
            let repeated = run_repeat(ctx, Side::Before, version, seeds, Kind::Fault(n))?;
            befores.push(ended(n, repeated));
        }
        let repeated = run_repeat(ctx, Side::Head, head.version, head.seeds, Kind::Fault(n))?;
        heads.push(ended(n, repeated));
    }
    Ok(fault::outcome(fault, heads, before.map(|_| befores)))
}

/// After the measured repeats of a head that passed them: its failure is a fact for the report,
/// not a failure of the run.
fn coverage<S: Sandbox>(ctx: &mut Ctx<S>, head: Passed) -> Result<CoverageOutcome, Stop> {
    let repeated = run_repeat(ctx, Side::Head, head.version, head.seeds, Kind::Coverage)?;
    Ok(match repeated.failure {
        Some(failure) => CoverageOutcome::Failed(failure),
        None => CoverageOutcome::Read(repeated.calls),
    })
}

/// One repeat (architecture plan, «Тома, снимки и деревья», steps 3 to 5): the services by
/// levels, the fault of a fault repeat once what the workload waits for is ready, the workload to
/// its exit, then the cut, the probes of a measured repeat (`after-workload`, `on-failure` when it
/// failed), the clean stop, the end-of-run snapshots of the last or a failed measured repeat, and
/// the drain, then the profiles on the host. Its deadline runs from its start, readiness included.
fn run_repeat<S: Sandbox>(
    ctx: &mut Ctx<S>,
    side: Side,
    version: &Version,
    seeds: &Seeds,
    kind: Kind,
) -> Result<Repeated, Stop> {
    if ctx.cancel.is_cancelled() {
        return Err(Stop::Cancelled(Phase::Ready));
    }
    let stand = ctx.stand;
    let (dir, timeout, mode, repeat, switch) = match kind {
        Kind::Measured(repeat) => (
            ctx.run_dir.join(format!("repeat-{repeat}-{side}")),
            stand.timeout.get(),
            Some(ProfileMode::Cpu),
            Some(repeat),
            None,
        ),
        Kind::Coverage => (
            ctx.run_dir.join("coverage"),
            stand.timeout.get() * 3 / 2,
            Some(ProfileMode::Coverage),
            None,
            None,
        ),
        Kind::Fault(repeat) => (
            ctx.run_dir.join(format!("fault-{repeat}-{side}")),
            stand.timeout.get(),
            None,
            Some(repeat),
            stand.fault.as_ref().map(fault::switch),
        ),
    };
    let out = dir.join("out");
    fs::create_dir_all(&out)
        .map_err(StandFault::io(&out))
        .map_err(|fault| Stop::Fault(Phase::Ready, fault))?;
    let mut fleet = Fleet::default();
    let deadline = ctx.sandbox.now() + timeout;
    let frame = Frame {
        out: &out,
        second: None,
        tar: &version.tar,
        seeds,
        mode,
        preload: &ctx.preload,
        span: ctx.span,
    };
    let Ran {
        phase,
        wall,
        failing,
    } = up_to_exit(ctx, &mut fleet, &frame, switch, deadline, &dir)?;
    let tap = match (kind, &failing) {
        (Kind::Coverage | Kind::Fault(_), _) => None,
        (Kind::Measured(_), Some(_)) => Some(Tap::OnFailure),
        (Kind::Measured(_), None) => Some(Tap::AfterWorkload),
    };
    let keeps = match kind {
        Kind::Measured(repeat) if failing.is_some() || repeat == stand.repeat.get() => Some(side),
        _ => None,
    };
    let ends = match kind {
        Kind::Fault(_) => {
            let ends = fault::ends(ctx.sandbox, &mut fleet).map_err(|error| {
                let fault = StandFault::Sandbox(error);
                torn(ctx.sandbox, &mut fleet, &dir, Stop::Fault(phase, fault))
            });
            Some(ends?)
        }
        Kind::Measured(_) | Kind::Coverage => None,
    };
    let mut ended = end_phase(ctx, &mut fleet, tap, keeps, &dir, &out)
        .map_err(|fault| torn(ctx.sandbox, &mut fleet, &dir, Stop::Fault(phase, fault)))?;
    let read = profiles(&dir, kind, failing.is_some(), &mut ended);
    let calls = read.map_err(|fault| Stop::Fault(phase, fault))?;
    let Ended {
        instances, probes, ..
    } = ended;
    let failure = failing.map(|failing| failure(failing, phase, repeat, &dir, &probes));
    let output = RepeatOutput {
        // 0 only in the coverage repeat, whose output never leaves `run`.
        repeat: repeat.unwrap_or_default(),
        wall,
        instances,
        probes,
        out_dir: out,
    };
    Ok(Repeated {
        output,
        failure,
        calls,
        ends,
    })
}

/// How a repeat ran up to the exit of its (first) workload, or to what stopped it first. The phase
/// is `Ready` until the `create` with the workload returned, `Workload` from then on; `wall` runs
/// from that return to the workload's exit.
struct Ran {
    phase: Phase,
    wall: Duration,
    failing: Option<Failing>,
}

fn up_to_exit<S: Sandbox>(
    ctx: &Ctx<S>,
    fleet: &mut Fleet,
    frame: &Frame,
    fault: Option<Fault>,
    deadline: Instant,
    dir: &Utf8Path,
) -> Result<Ran, Stop> {
    let mut started = None;
    let ran = start_repeat(ctx, fleet, frame, fault, deadline, &mut started)
        .and_then(|()| fleet.finished(ctx.sandbox, deadline, ctx.cancel));
    // The workload is a member once its `create` returned, also when the fault after it failed.
    let phase = match fleet.workload() {
        Some(_) => Phase::Workload,
        None => Phase::Ready,
    };
    let (ended, failing) = workload_outcome(ctx.sandbox, ctx.stand, fleet, ran, phase, dir)?;
    let wall = started.map_or(Duration::ZERO, |started| {
        ended.saturating_duration_since(started)
    });
    Ok(Ran {
        phase,
        wall,
        failing,
    })
}

/// `started`: when the `create` with the workload returned. `fault` goes on once what the
/// workload waits for is ready, before its batch.
fn start_repeat<S: Sandbox>(
    ctx: &Ctx<S>,
    fleet: &mut Fleet,
    frame: &Frame,
    mut fault: Option<Fault>,
    deadline: Instant,
    started: &mut Option<Instant>,
) -> Result<(), Halt> {
    let stand = ctx.stand;
    for level in launch::levels(stand, |_, service| service.kind != ServiceKind::Setup) {
        for name in &level {
            wait_for(ctx, fleet, name, deadline)?;
        }
        let workload = level.contains(&&stand.workload);
        if workload && let Some(fault) = fault.take() {
            fleet.fault_on(ctx.sandbox, fault)?;
        }
        let created = fleet.start(ctx.sandbox, launch::starts(stand, &level, frame))?;
        if workload {
            *started = Some(created);
        }
    }
    Ok(())
}

/// The service and the cause of a failed repeat.
type Failing = (Option<String>, Cause);

/// When the wait for the workload ended, and what failed if anything did; `Err` when the run
/// itself stops instead, its members torn down.
fn workload_outcome(
    sandbox: &impl Sandbox,
    stand: &Stand,
    fleet: &mut Fleet,
    ran: Result<(Option<i32>, Instant), Halt>,
    phase: Phase,
    dir: &Utf8Path,
) -> Result<(Instant, Option<Failing>), Stop> {
    // A long-running service that ended with the workload ends the repeat it would have measured;
    // after another exit of the workload, that exit is the cause.
    let ran = match ran {
        Ok((Some(0), at)) => fleet.settled(sandbox).map(|()| (Some(0), at)),
        ran => ran,
    };
    Ok(match ran {
        Ok((Some(0), at)) => (at, None),
        Ok((code, at)) => {
            let workload = Some(stand.workload.as_str().to_owned());
            (at, Some((workload, code.map_or(Cause::Died, Cause::Exit))))
        }
        Err(halt) => {
            let fell = fell(fleet, halt, Cause::Timeout, phase)
                .map_err(|stop| torn(sandbox, fleet, dir, stop))?;
            (sandbox.now(), Some(fell))
        }
    })
}

fn failure(
    (service, cause): Failing,
    phase: Phase,
    repeat: Option<u32>,
    dir: &Utf8Path,
    probes: &[ProbeRan],
) -> Failure {
    Failure {
        phase,
        service,
        repeat,
        cause,
        logs: Some(dir.to_owned()),
        probes: probes.iter().map(|probe| probe.out.clone()).collect(),
    }
}

/// How far a second pass got.
enum Reached {
    /// The first pass failed.
    First(Failing),
    /// The second pass ran after the first passed, and failed or not.
    Second(Pass, Option<Failing>),
}

/// The workload twice on one state (architecture plan, «Тома, снимки и деревья», step 6): a
/// repeat as a measured one, without profiler and kept snapshot, under `timeout` x 2. After the
/// first pass the cut, its probes into `out-1/` and the end of the first workload VM; then the
/// workload again in a fresh VM on `out-2/`, beside the same volumes and long-running services,
/// no restore. A failure in either pass ends the repeat as a failed measured one does; it is a
/// fact, never a failure of the run.
fn second_pass<S: Sandbox>(ctx: &mut Ctx<S>, side: Side, passed: Passed) -> Result<PassPair, Stop> {
    if ctx.cancel.is_cancelled() {
        return Err(Stop::Cancelled(Phase::Ready));
    }
    let (stand, sandbox) = (ctx.stand, ctx.sandbox);
    let dir = ctx.run_dir.join(format!("pass2-{side}"));
    // One per pass: its workload binds it, a service with a probe both (`Frame.second`), so both
    // are there before the first VM. The host writes its own files beside them.
    let outs = [dir.join("out-1"), dir.join("out-2")];
    for out in &outs {
        fs::create_dir_all(out)
            .map_err(StandFault::io(out))
            .map_err(|fault| Stop::Fault(Phase::Ready, fault))?;
    }
    let mut fleet = Fleet::default();
    let deadline = sandbox.now() + stand.timeout.get() * 2;
    let frame = |out| Frame {
        out,
        second: Some(&outs[1]),
        tar: &passed.version.tar,
        seeds: passed.seeds,
        mode: None,
        preload: &ctx.preload,
        span: ctx.span,
    };
    let first = up_to_exit(ctx, &mut fleet, &frame(&outs[0]), None, deadline, &dir)?;
    let phase = first.phase;
    let reached = match first.failing {
        Some(failing) => Reached::First(failing),
        None => {
            let first = between_passes(ctx, &mut fleet, &dir, &outs[0])
                .map_err(|fault| torn(sandbox, &mut fleet, &dir, Stop::Fault(phase, fault)))?;
            let again = launch::starts(stand, &[&stand.workload], &frame(&outs[1]));
            let ran = fleet
                .start(sandbox, again)
                .and_then(|_| fleet.finished(sandbox, deadline, ctx.cancel));
            let (_, failing) = workload_outcome(sandbox, stand, &mut fleet, ran, phase, &dir)?;
            Reached::Second(first, failing)
        }
    };
    let (tap, out) = match &reached {
        Reached::First(_) => (Tap::OnFailure, &outs[0]),
        Reached::Second(_, Some(_)) => (Tap::OnFailure, &outs[1]),
        Reached::Second(_, None) => (Tap::AfterWorkload, &outs[1]),
    };
    let ended = end_phase(ctx, &mut fleet, Some(tap), None, &dir, out)
        .map_err(|fault| torn(sandbox, &mut fleet, &dir, Stop::Fault(phase, fault)))?;
    let Ended {
        instances, probes, ..
    } = ended;
    Ok(match reached {
        Reached::First(failing) | Reached::Second(_, Some(failing)) => {
            PassPair::Failed(failure(failing, phase, None, &dir, &probes))
        }
        Reached::Second(first, None) => {
            let metrics = instances.into_iter().map(|instance| instance.metrics);
            PassPair::Passed {
                first,
                second: pass(out, probes, metrics),
            }
        }
    })
}

/// After the first pass: the metrics of every instance before any probe, its `after-workload`
/// probes into `out`, then the first workload VM drained and destroyed before the second one
/// starts, since the adapter keeps one host port per (service, replica, port). Its log is
/// `<workload>-1.pass-1.log` in `dir`.
fn between_passes<S: Sandbox>(
    ctx: &Ctx<S>,
    fleet: &mut Fleet,
    dir: &Utf8Path,
    out: &Utf8Path,
) -> Result<Pass, StandFault> {
    let sandbox = ctx.sandbox;
    let mut metrics = Vec::new();
    for member in &fleet.members {
        metrics.push(sandbox.metrics(&member.id).map_err(StandFault::Sandbox)?);
    }
    let tap = Tap::AfterWorkload;
    let probes = probes::run(sandbox, ctx.stand, fleet, tap, out).map_err(StandFault::Sandbox)?;
    let index = fleet
        .workload()
        .expect("the first pass started its workload");
    let log = drain::log_path(dir, &fleet.members[index]).with_extension("pass-1.log");
    drain::release(sandbox, fleet, index, &log)?;
    Ok(pass(out, probes, metrics))
}

/// `metrics` of every instance at the cut after the pass.
fn pass(out: &Utf8Path, probes: Vec<ProbeRan>, metrics: impl IntoIterator<Item = Metrics>) -> Pass {
    let ports = metrics.into_iter().flat_map(|metrics| metrics.ports);
    Pass {
        out_dir: out.to_owned(),
        probes,
        ports: ports.collect(),
    }
}

struct Ended {
    instances: Vec<InstanceOutput>,
    probes: Vec<ProbeRan>,
    /// Indexes into `instances`: the drain brought their chunks out.
    profiled: Vec<usize>,
}

/// The end of a repeat or of a failed setup. `keeps`: the side whose end-of-run snapshots this
/// repeat takes.
fn end_phase<S: Sandbox>(
    ctx: &mut Ctx<S>,
    fleet: &mut Fleet,
    tap: Option<Tap>,
    keeps: Option<Side>,
    dir: &Utf8Path,
    out: &Utf8Path,
) -> Result<Ended, StandFault> {
    let sandbox = ctx.sandbox;
    let cut = drain::cut(sandbox, fleet).map_err(StandFault::Sandbox)?;
    let probes = match tap {
        Some(tap) => {
            probes::run(sandbox, ctx.stand, fleet, tap, out).map_err(StandFault::Sandbox)?
        }
        None => Vec::new(),
    };
    let stopped = drain::stop(sandbox, fleet).map_err(StandFault::Sandbox)?;
    if let Some(side) = keeps {
        keep::take(sandbox, fleet, side, &mut ctx.kept)?;
    }
    drain::write_metrics(dir, fleet, &cut, &stopped)?;
    let profiled = fleet.members.iter().enumerate();
    let profiled = profiled.filter(|(_, member)| drain::is_profiled(member));
    let profiled = profiled.map(|(index, _)| index).collect();
    let instances = fleet.members.iter().zip(cut).enumerate();
    let instances = instances.map(|(index, (member, cut))| InstanceOutput {
        service: member.launch.service.clone(),
        replica: member.launch.replica,
        role: match member.kind {
            ServiceKind::Workload => Role::Workload,
            ServiceKind::LongRunning | ServiceKind::Setup => Role::LongRunning,
        },
        metrics: cut.metrics,
        log: drain::log_path(dir, member),
        log_cut: cut.log,
        stopped: stopped.get(&index).copied(),
        profile: None,
    });
    let instances = instances.collect();
    drain::drain(sandbox, fleet, dir)?;
    Ok(Ended {
        instances,
        probes,
        profiled,
    })
}

/// Once the VMs are gone: the samples of each profiled instance of a measured repeat, the calls of
/// each profiled service of a coverage repeat that passed.
fn profiles(
    dir: &Utf8Path,
    kind: Kind,
    failed: bool,
    ended: &mut Ended,
) -> Result<BTreeMap<String, Coverage>, StandFault> {
    let mut calls: BTreeMap<String, Vec<Coverage>> = BTreeMap::new();
    for &index in &ended.profiled {
        let instance = &mut ended.instances[index];
        let (service, replica) = (instance.service.as_str(), instance.replica);
        match kind {
            Kind::Measured(_) => {
                instance.profile = Some(drain::samples(dir, service, replica)?);
            }
            // No profiler runs in a fault repeat.
            Kind::Fault(_) => {}
            Kind::Coverage if failed => {}
            Kind::Coverage => {
                let part = drain::coverage(dir, service, replica)?;
                calls.entry(service.to_owned()).or_default().push(part);
            }
        }
    }
    let merged = calls
        .into_iter()
        .map(|(service, parts)| (service, merged(parts)));
    Ok(merged.collect())
}

/// The replicas of a service as one: the calls of a function add up.
fn merged(parts: Vec<Coverage>) -> Coverage {
    let mut functions: BTreeMap<(Utf8PathBuf, u32, u32), Called> = BTreeMap::new();
    for called in parts.into_iter().flat_map(|part| part.called) {
        match functions.entry((called.file.clone(), called.line, called.column)) {
            Entry::Occupied(mut entry) => {
                let calls = &mut entry.get_mut().calls;
                *calls = calls.saturating_add(called.calls);
            }
            Entry::Vacant(entry) => {
                entry.insert(called);
            }
        }
    }
    Coverage {
        called: functions.into_values().collect(),
    }
}

/// What a halt says about the app; `Err` when the run itself stops instead.
fn fell(fleet: &Fleet, halt: Halt, timeout: Cause, phase: Phase) -> Result<Failing, Stop> {
    let service = |index: usize| Some(fleet.members[index].launch.service.clone());
    match halt {
        Halt::Down(index) => {
            let member = &fleet.members[index];
            let code = member.exit.and_then(|(code, _)| code);
            let cause = match (member.kind, code) {
                (ServiceKind::LongRunning, _) | (_, None) => Cause::Died,
                (_, Some(code)) => Cause::Exit(code),
            };
            Ok((service(index), cause))
        }
        Halt::Unhealthy(index) => Ok((service(index), Cause::Unhealthy)),
        Halt::Deadline(index) => Ok((service(index), timeout)),
        Halt::Cancelled => Err(Stop::Cancelled(phase)),
        Halt::Port(error) => Err(Stop::Fault(phase, StandFault::Sandbox(error))),
    }
}

/// A stop with members alive: their logs are read where they can be, then they are destroyed.
fn torn(sandbox: &impl Sandbox, fleet: &mut Fleet, dir: &Utf8Path, stop: Stop) -> Stop {
    drain::teardown(sandbox, fleet, dir);
    stop
}

impl<'r, S: Sandbox> Ctx<'r, S> {
    fn new(
        stand: &'r Stand,
        sandbox: &'r S,
        extras: &'r BTreeSet<Extra>,
        run_dir: &'r Utf8Path,
        cancel: &'r Cancel,
    ) -> Result<Ctx<'r, S>, StandFault> {
        let preload = run_dir.join("preload");
        if launch::any_profiled(stand) {
            fs::create_dir_all(&preload).map_err(StandFault::io(&preload))?;
            let file = preload.join("preload.cjs");
            fs::write(&file, profile::PRELOAD).map_err(StandFault::io(&file))?;
        }
        Ok(Ctx {
            stand,
            sandbox,
            extras,
            run_dir,
            cancel,
            span: span(stand),
            preload,
            loose: Vec::new(),
            kept: Vec::new(),
        })
    }

    /// `kept.json` lists what was kept, and the setup snapshots no entry took go. A kept snapshot
    /// `kept.json` could not list goes too: no list would ever name it.
    fn end(&mut self) -> Result<(), StandFault> {
        let written = keep::write(self.run_dir, &self.kept);
        let mut gone = std::mem::take(&mut self.loose);
        if written.is_err() {
            gone.extend(self.kept.drain(..).map(|kept| kept.snapshot));
        }
        let mut dropped = Ok(());
        for snapshot in gone {
            if let Err(error) = self.sandbox.drop_snapshot(&snapshot) {
                dropped = dropped.and(Err(StandFault::Sandbox(error)));
            }
        }
        written.and(dropped)
    }
}
