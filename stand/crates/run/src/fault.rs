//! The fault repeats (architecture plan, row 73): once what the workload waits for is ready, the
//! relays of the repeat refuse or delay the target, and how the repeat ended is a category for
//! the report, never a failure of the run.

use std::time::Duration;

use report::{FaultOutcome, FaultRepeat};
use sandbox::{Fault, FaultMode, Sandbox, SandboxError};
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

/// How a fault repeat ended, read before its stop ends every service.
pub(crate) struct Ends {
    pub(crate) workload: Option<i32>,
    pub(crate) exited: Vec<String>,
}

pub(crate) fn ends(sandbox: &impl Sandbox, fleet: &mut Fleet) -> Result<Ends, SandboxError> {
    fleet.ended(sandbox)?;
    let workload = fleet.workload().and_then(|index| fleet.members[index].exit);
    let mut exited: Vec<String> = Vec::new();
    for member in &fleet.members {
        let service = &member.launch.service;
        let ended = member.kind == ServiceKind::LongRunning && member.exit.is_some();
        if ended && !exited.contains(service) {
            exited.push(service.clone());
        }
    }
    Ok(Ends {
        workload: workload.and_then(|(code, _)| code),
        exited,
    })
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
            spec::FaultMode::Slow(delay) => format!("slow {}ms", millis(delay)),
        },
        head,
        before,
    }
}

/// `2s` is `2000`, `1500us` is `1.5`: whole milliseconds would turn a finer delay into another.
fn millis(delay: Duration) -> String {
    let nanos = delay.as_nanos();
    let (whole, rest) = (nanos / 1_000_000, nanos % 1_000_000);
    if rest == 0 {
        return whole.to_string();
    }
    let fraction = format!("{rest:06}");
    format!("{whole}.{}", fraction.trim_end_matches('0'))
}
