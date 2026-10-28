//! The fault repeats (architecture plan, row 73): once what the workload waits for is ready, the
//! relays of the repeat refuse or delay the target, and how the repeat ended is a category for
//! the report, never a failure of the run.

use report::{FaultOutcome, FaultRepeat, InstanceOutput};
use sandbox::{Fault, FaultMode, Sandbox, SandboxError, SandboxId};
use spec::ServiceKind;

use crate::ready::Fleet;

/// Per version.
pub(crate) const REPEATS: u32 = 2;

/// The relays know a target by the name of its service, as the peers do.
pub(crate) fn switch(fault: &spec::Fault) -> Fault {
    Fault {
        target: target(fault),
        mode: match fault.mode {
            spec::FaultMode::Down => FaultMode::Down,
            spec::FaultMode::Slow(delay) => FaultMode::Slow(delay),
        },
    }
}

fn target(fault: &spec::Fault) -> String {
    let target = &fault.target;
    format!("{}:{}", target.service.as_str(), target.port.get())
}

/// The members from `from` on get the fault, the workload aside: it drives the repeat, and the
/// fault is on what the app does. From now on a long-running member that ends does not halt.
pub(crate) fn on(
    sandbox: &impl Sandbox,
    fleet: &mut Fleet,
    from: usize,
    fault: &Fault,
) -> Result<(), SandboxError> {
    fleet.under_fault = true;
    let members = fleet.members[from..].iter();
    let ids: Vec<SandboxId> = members
        .filter(|member| member.kind != ServiceKind::Workload)
        .map(|member| member.id.clone())
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    sandbox.fault(&ids, fault)
}

/// How a fault repeat ended, read before its stop ends every service.
pub(crate) struct Ends {
    workload: Option<i32>,
    exited: Vec<String>,
}

pub(crate) fn ends(fleet: &Fleet) -> Ends {
    let workload = fleet
        .members
        .iter()
        .find(|member| member.kind == ServiceKind::Workload)
        .and_then(|member| member.exit)
        .and_then(|(code, _)| code);
    let mut exited: Vec<String> = Vec::new();
    for member in &fleet.members {
        let service = &member.launch.service;
        let ended = member.kind == ServiceKind::LongRunning && member.exit.is_some();
        if ended && !exited.contains(service) {
            exited.push(service.clone());
        }
    }
    Ends { workload, exited }
}

pub(crate) fn repeat(repeat: u32, ends: Ends, instances: Vec<InstanceOutput>) -> FaultRepeat {
    FaultRepeat {
        repeat,
        workload: ends.workload,
        exited: ends.exited,
        instances,
    }
}

pub(crate) fn outcome(
    fault: &spec::Fault,
    head: Vec<FaultRepeat>,
    before: Option<Vec<FaultRepeat>>,
) -> FaultOutcome {
    FaultOutcome {
        target: target(fault),
        mode: match fault.mode {
            spec::FaultMode::Down => "down".to_owned(),
            spec::FaultMode::Slow(delay) => format!("slow {}ms", delay.as_millis()),
        },
        head,
        before,
    }
}
