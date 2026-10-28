//! The probes of one tap (architecture plan, row 62): each in replica 1 of its service, through
//! `exec` under `PROBE`, stdout into `<out>/probe.<name>.jsonl`, stderr beside it in `.err`. A
//! code other than 0 or the deadline is what the probe saw, not a failure of the run.

use std::time::Duration;

use camino::Utf8Path;
use report::ProbeRan;
use sandbox::{Sandbox, SandboxError};
use spec::{ProbeName, Stand, Tap};

use crate::ready::Fleet;

const PROBE: Duration = Duration::from_secs(30);

/// A probe whose service is not in `fleet` does not run: an `on-failure` probe of a service that
/// never started. One output file per probe: the replicas share the state below the app.
pub(crate) fn run(
    sandbox: &impl Sandbox,
    stand: &Stand,
    fleet: &Fleet,
    tap: Tap,
    out: &Utf8Path,
) -> Result<Vec<ProbeRan>, SandboxError> {
    let mut ran = Vec::new();
    for (name, probe) in &stand.probes {
        if probe.at != tap {
            continue;
        }
        let Some(index) = fleet.find(probe.service.as_str(), 1) else {
            continue;
        };
        let file = out.join(file_name(name));
        let deadline = sandbox.now() + PROBE;
        let outcome = sandbox.exec(&fleet.members[index].id, &probe.run, deadline, &file)?;
        ran.push(ProbeRan {
            name: name.as_str().to_owned(),
            outcome,
            out: file,
        });
    }
    Ok(ran)
}

pub(crate) fn file_name(name: &ProbeName) -> String {
    format!("probe.{}.jsonl", name.as_str())
}
