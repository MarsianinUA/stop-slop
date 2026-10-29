//! The repeats outside the stats (architecture plan, row 89), after the measured repeats of a head
//! that passed them. How each ended is a fact for the report, never a failure of the run.

use std::fs;

use camino::Utf8Path;
use report::{
    CoverageOutcome, Extra, FaultOutcome, FaultRepeat, Pass, PassPair, Phase, ProbeRan, Role,
    SecondPassOutcome,
};
use sandbox::{PortStats, Sandbox};
use spec::{ServiceKind, Tap};

use super::{
    Ctx, Ended, Failing, Kind, Repeated, Stop, end_phase, failure, run_repeat, torn, up_to_exit,
    workload_outcome,
};
use crate::launch::{self, Frame, Seeds};
use crate::ready::Fleet;
use crate::{Side, StandFault, Version, drain, fault, probes};

#[derive(Default)]
pub(super) struct Outside {
    pub(super) coverage: Option<CoverageOutcome>,
    pub(super) fault: Option<FaultOutcome>,
    pub(super) second_pass: Option<SecondPassOutcome>,
}

/// A version whose lane passed its measured repeats, and what they start from.
#[derive(Clone, Copy)]
pub(super) struct Passed<'a> {
    pub(super) version: &'a Version,
    pub(super) seeds: &'a Seeds,
}

/// Each repeat outside the stats that `extras` names and the stand asks for: the coverage repeat
/// of the head, the fault repeats, the second pass of each version, "before" first. `before`
/// takes part only when its lane passed too.
pub(super) fn outside<S: Sandbox>(
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

fn coverage<S: Sandbox>(ctx: &mut Ctx<S>, head: Passed) -> Result<CoverageOutcome, Stop> {
    let repeated = run_repeat(ctx, Side::Head, head.version, head.seeds, Kind::Coverage)?;
    Ok(match repeated.failure {
        Some(failure) => CoverageOutcome::Failed(failure),
        None => CoverageOutcome::Read(repeated.calls),
    })
}

/// Two per version in turns, "before" first (architecture plan, «Тома, снимки и деревья», step 5).
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
/// no restore. A failure in either pass ends the repeat as a failed measured one does.
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
            let services = instances
                .into_iter()
                .filter(|instance| instance.role == Role::LongRunning);
            let ports = services.flat_map(|instance| instance.metrics.ports);
            PassPair::Passed {
                first,
                second: pass(out, probes, ports),
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
    let mut ports = Vec::new();
    for member in &fleet.members {
        let metrics = sandbox.metrics(&member.id).map_err(StandFault::Sandbox)?;
        if member.kind == ServiceKind::LongRunning {
            ports.extend(metrics.ports);
        }
    }
    let tap = Tap::AfterWorkload;
    let probes = probes::run(sandbox, ctx.stand, fleet, tap, out).map_err(StandFault::Sandbox)?;
    let index = fleet
        .workload()
        .expect("the first pass started its workload");
    let log = drain::log_path(dir, &fleet.members[index]).with_extension("pass-1.log");
    drain::release(sandbox, fleet, index, &log)?;
    Ok(pass(out, probes, ports))
}

/// `ports` of the long-running services at the cut after the pass, without the workload's
/// (`Pass.ports`).
fn pass(out: &Utf8Path, probes: Vec<ProbeRan>, ports: impl IntoIterator<Item = PortStats>) -> Pass {
    Pass {
        out_dir: out.to_owned(),
        probes,
        ports: ports.into_iter().collect(),
    }
}
