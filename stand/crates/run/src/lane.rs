//! The lane of each version (architecture plan, row 10) and the steps of one repeat. Each version
//! is set up, "before" first, then the measured repeats go in turns, "before" first. A failure of
//! "before" stops its lane only; a failure of the head ends the run. Only one phase has VMs at a
//! time, so no two live VMs share a service and a replica: the adapter keeps one host port per
//! (service, replica, port) (plan T-11, decision 7).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::time::{Duration, Instant};

use camino::Utf8Path;
use report::{
    BeforeOutcome, Cause, Failure, InstanceOutput, Phase, ProbeRule, Raw, RepeatOutput, Role,
    Setup, SetupSide,
};
use sandbox::{Sandbox, SnapshotId};
use spec::{Condition, ServiceKind, ServiceName, Stand};

use crate::drain;
use crate::launch::{self, Frame, Seeds};
use crate::ready::{Fleet, Halt, Member};
use crate::{Cancel, Failed, RunError, Side, StandFault, Version, Versions, probes, span};

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

struct Ctx<'r, S> {
    stand: &'r Stand,
    sandbox: &'r S,
    run_dir: &'r Utf8Path,
    cancel: &'r Cancel,
    span: Duration,
    /// Setup snapshots no cache entry holds: they go at the end.
    loose: Vec<SnapshotId>,
}

pub(crate) fn run<S: Sandbox>(
    stand: &Stand,
    sandbox: &S,
    versions: &Versions,
    run_dir: &Utf8Path,
    cancel: &Cancel,
) -> Result<Raw, RunError> {
    let mut ctx = Ctx {
        stand,
        sandbox,
        run_dir,
        cancel,
        span: span(stand),
        loose: Vec::new(),
    };
    let result = lanes(&mut ctx, versions);
    // On every path; after another error its own error is dropped, the first one ends the run.
    let end = ctx.end();
    let raw = result?;
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
    let (outputs, failure) = match head {
        Lane::Ready { outputs, .. } => (outputs, None),
        Lane::Failed { failure, outputs } => (outputs, Some(failure)),
    };
    let raw = raw(ctx.stand, head_setup, outputs, before_setup, before);
    match failure {
        None => Ok(raw),
        Some(failure) => Err(RunError::App(Box::new(Failed {
            failure,
            raw,
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
        coverage: None,
    }
}

fn set_up<S: Sandbox>(
    ctx: &mut Ctx<S>,
    side: Side,
    version: &Version,
) -> Result<(SetupSide, Lane), Stop> {
    if ctx.cancel.is_cancelled() {
        return Err(Stop::Cancelled(Phase::Setup));
    }
    let dir = ctx.run_dir.join(format!("setup-{side}"));
    let out = dir.join("out");
    fs::create_dir_all(&out)
        .map_err(StandFault::io(&out))
        .map_err(|fault| Stop::Fault(Phase::Setup, fault))?;
    let (setup, built) = build(ctx, version, &dir, &out)?;
    let outputs = Vec::new();
    let lane = match built {
        Ok(seeds) => Lane::Ready { seeds, outputs },
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
    let side = |sizes| SetupSide {
        sizes,
        probes: Vec::new(),
        out_dir: out.to_owned(),
    };
    let halt = match start_setup(ctx, &mut fleet, &mut seeds, version, out, deadline) {
        Ok(()) => {
            let sizes = finish_setup(ctx, &mut fleet, &mut seeds, dir).map_err(|fault| {
                torn(
                    ctx.sandbox,
                    &mut fleet,
                    dir,
                    Stop::Fault(Phase::Setup, fault),
                )
            })?;
            return Ok((side(sizes), Ok(seeds)));
        }
        Err(halt) => halt,
    };
    let (service, cause) = fell(&fleet, halt, Cause::SetupTimeout, Phase::Setup)
        .map_err(|stop| torn(ctx.sandbox, &mut fleet, dir, stop))?;
    fail_setup(ctx, &mut fleet, &seeds, dir).map_err(|fault| {
        torn(
            ctx.sandbox,
            &mut fleet,
            dir,
            Stop::Fault(Phase::Setup, fault),
        )
    })?;
    let failure = Failure {
        phase: Phase::Setup,
        service,
        repeat: None,
        cause,
        logs: Some(dir.to_owned()),
        probes: Vec::new(),
    };
    Ok((side(BTreeMap::new()), Err(failure)))
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
    let in_setup = launch::in_setup(stand);
    for level in launch::levels(stand, |name, _| in_setup.contains(name)) {
        for name in &level {
            wait_for(ctx, fleet, name, deadline)?;
        }
        version_snapshot(ctx, fleet, seeds)?;
        let frame = Frame {
            out,
            tar: &version.tar,
            seeds,
            span: ctx.span,
        };
        fleet.start(ctx.sandbox, launch::starts(stand, &level, &frame))?;
    }
    for index in 0..fleet.members.len() {
        if fleet.members[index].kind == ServiceKind::Setup {
            fleet.completed(ctx.sandbox, index, deadline, ctx.cancel)?;
        }
    }
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

/// After every setup service exited 0: the cut, the clean stop, the sizes and the snapshot of
/// each VM with a volume, the drain. `metrics.json` of setup shows what the prepare service took
/// of its limits; no measure reads it.
fn finish_setup<S: Sandbox>(
    ctx: &mut Ctx<S>,
    fleet: &mut Fleet,
    seeds: &mut Seeds,
    dir: &Utf8Path,
) -> Result<BTreeMap<String, u64>, StandFault> {
    let (stand, sandbox) = (ctx.stand, ctx.sandbox);
    let cut = drain::cut(sandbox, fleet).map_err(StandFault::Sandbox)?;
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
        seeds.volumes.insert(service_name(stand, member), snapshot);
    }
    drain::write_metrics(dir, fleet, &cut, &stopped)?;
    drain::drain(sandbox, fleet, dir)?;
    Ok(sizes)
}

/// A failed setup: the cut, the clean stop, the drain; the snapshots it took go at once.
fn fail_setup<S: Sandbox>(
    ctx: &mut Ctx<S>,
    fleet: &mut Fleet,
    seeds: &Seeds,
    dir: &Utf8Path,
) -> Result<(), StandFault> {
    let sandbox = ctx.sandbox;
    let cut = drain::cut(sandbox, fleet).map_err(StandFault::Sandbox)?;
    let stopped = drain::stop(sandbox, fleet).map_err(StandFault::Sandbox)?;
    drain::write_metrics(dir, fleet, &cut, &stopped)?;
    drain::drain(sandbox, fleet, dir)?;
    for snapshot in seeds.snapshots() {
        sandbox
            .drop_snapshot(snapshot)
            .map_err(StandFault::Sandbox)?;
        ctx.loose.retain(|loose| loose != snapshot);
    }
    Ok(())
}

/// Repeat `repeat` of `lane`, unless it already failed; a failed repeat turns it `Failed`.
fn measure<S: Sandbox>(
    ctx: &Ctx<S>,
    side: Side,
    version: &Version,
    lane: &mut Lane,
    repeat: u32,
) -> Result<(), Stop> {
    let Lane::Ready { seeds, outputs } = lane else {
        return Ok(());
    };
    let (output, failure) = run_repeat(ctx, side, version, seeds, repeat)?;
    outputs.push(output);
    if let Some(failure) = failure {
        let outputs = std::mem::take(outputs);
        *lane = Lane::Failed { failure, outputs };
    }
    Ok(())
}

/// One measured repeat (architecture plan, «Тома, снимки и деревья», steps 3 and 4): the services
/// by levels, the workload to its exit, then the cut, the clean stop and the drain, also when it
/// failed. Its deadline is `timeout` from its start, readiness included. The phase is `Ready`
/// until the `create` with the workload returned, `Workload` from then on.
fn run_repeat<S: Sandbox>(
    ctx: &Ctx<S>,
    side: Side,
    version: &Version,
    seeds: &Seeds,
    repeat: u32,
) -> Result<(RepeatOutput, Option<Failure>), Stop> {
    if ctx.cancel.is_cancelled() {
        return Err(Stop::Cancelled(Phase::Ready));
    }
    let stand = ctx.stand;
    let dir = ctx.run_dir.join(format!("repeat-{repeat}-{side}"));
    let out = dir.join("out");
    fs::create_dir_all(&out)
        .map_err(StandFault::io(&out))
        .map_err(|fault| Stop::Fault(Phase::Ready, fault))?;
    let mut fleet = Fleet::default();
    let deadline = ctx.sandbox.now() + stand.timeout.get();
    let frame = Frame {
        out: &out,
        tar: &version.tar,
        seeds,
        span: ctx.span,
    };
    let mut started = None;
    let ran = start_repeat(ctx, &mut fleet, &frame, deadline, &mut started)
        .and_then(|()| fleet.finished(ctx.sandbox, deadline, ctx.cancel));
    let phase = match started {
        Some(_) => Phase::Workload,
        None => Phase::Ready,
    };
    let (ended, failing) = match ran {
        Ok((Some(0), at)) => (at, None),
        Ok((code, at)) => {
            let workload = Some(stand.workload.as_str().to_owned());
            (at, Some((workload, code.map_or(Cause::Died, Cause::Exit))))
        }
        Err(halt) => {
            let fell = fell(&fleet, halt, Cause::Timeout, phase)
                .map_err(|stop| torn(ctx.sandbox, &mut fleet, &dir, stop))?;
            (ctx.sandbox.now(), Some(fell))
        }
    };
    let wall = started.map_or(Duration::ZERO, |started| {
        ended.saturating_duration_since(started)
    });
    let instances = end_repeat(ctx.sandbox, &mut fleet, &dir)
        .map_err(|fault| torn(ctx.sandbox, &mut fleet, &dir, Stop::Fault(phase, fault)))?;
    let failure = failing.map(|(service, cause)| Failure {
        phase,
        service,
        repeat: Some(repeat),
        cause,
        logs: Some(dir.clone()),
        probes: Vec::new(),
    });
    let output = RepeatOutput {
        repeat,
        wall,
        instances,
        probes: Vec::new(),
        out_dir: out,
    };
    Ok((output, failure))
}

/// `started`: when the `create` with the workload returned, the start of `wall`.
fn start_repeat<S: Sandbox>(
    ctx: &Ctx<S>,
    fleet: &mut Fleet,
    frame: &Frame,
    deadline: Instant,
    started: &mut Option<Instant>,
) -> Result<(), Halt> {
    let stand = ctx.stand;
    for level in launch::levels(stand, |_, service| service.kind != ServiceKind::Setup) {
        for name in &level {
            wait_for(ctx, fleet, name, deadline)?;
        }
        let created = fleet.start(ctx.sandbox, launch::starts(stand, &level, frame))?;
        if level.contains(&&stand.workload) {
            *started = Some(created);
        }
    }
    Ok(())
}

fn end_repeat(
    sandbox: &impl Sandbox,
    fleet: &mut Fleet,
    dir: &Utf8Path,
) -> Result<Vec<InstanceOutput>, StandFault> {
    let cut = drain::cut(sandbox, fleet).map_err(StandFault::Sandbox)?;
    let stopped = drain::stop(sandbox, fleet).map_err(StandFault::Sandbox)?;
    drain::write_metrics(dir, fleet, &cut, &stopped)?;
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
    Ok(instances)
}

/// What a halt says about the app; `Err` when the run itself stops instead.
fn fell(
    fleet: &Fleet,
    halt: Halt,
    timeout: Cause,
    phase: Phase,
) -> Result<(Option<String>, Cause), Stop> {
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

fn service_name(stand: &Stand, member: &Member) -> ServiceName {
    let names = stand.services.keys();
    let mut names = names.filter(|name| name.as_str() == member.launch.service);
    names
        .next()
        .expect("every member is a service of the stand")
        .clone()
}

impl<S: Sandbox> Ctx<'_, S> {
    /// The setup snapshots no entry took go.
    fn end(&mut self) -> Result<(), StandFault> {
        let mut dropped = Ok(());
        for snapshot in self.loose.drain(..) {
            if let Err(error) = self.sandbox.drop_snapshot(&snapshot) {
                dropped = dropped.and(Err(StandFault::Sandbox(error)));
            }
        }
        dropped
    }
}
