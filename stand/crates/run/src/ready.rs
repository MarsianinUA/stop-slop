//! The VMs of one phase and what `run` waits for on them: the exit of a setup service or of the
//! workload, a passed healthcheck, a deadline. Every wait goes in chunks of at most 1 s with
//! `Cancel` between them, and every running VM of the phase is in it, so a neighbour that dies
//! during a healthcheck attempt shows at once (architecture plan, port `Sandbox`).

use std::time::{Duration, Instant};

use sandbox::{ExecId, Launch, Sandbox, SandboxError, SandboxId, Waitable};
use spec::{Healthcheck, ServiceKind};

use crate::Cancel;

const CHUNK: Duration = Duration::from_secs(1);

/// A member to start: its launch and what the waits need to know of it.
pub(crate) struct Start {
    pub(crate) kind: ServiceKind,
    pub(crate) launch: Launch,
    pub(crate) health: Option<Healthcheck>,
}

pub(crate) struct Member {
    pub(crate) kind: ServiceKind,
    pub(crate) launch: Launch,
    pub(crate) health: Option<Healthcheck>,
    pub(crate) id: SandboxId,
    pub(crate) created: Instant,
    /// How its command ended: the code (`None` when killed without one) and when.
    pub(crate) exit: Option<(Option<i32>, Instant)>,
    pub(crate) healthy: bool,
}

/// The VMs of one setup, repeat or inspect, in the order they were created, so the reverse is
/// dependents first.
#[derive(Default)]
pub(crate) struct Fleet {
    pub(crate) members: Vec<Member>,
    /// A fault repeat once its fault is on: a long-running service that ends is one of its
    /// categories, noted in its `exit`, and the waits go on.
    pub(crate) under_fault: bool,
}

/// Why a phase stopped short of its goal.
pub(crate) enum Halt {
    /// A member's command ended: a long-running service (not under a fault), or a setup service
    /// with another code than 0.
    Down(usize),
    Unhealthy(usize),
    /// The phase's deadline, while waiting for this member.
    Deadline(usize),
    Cancelled,
    Port(SandboxError),
}

impl From<SandboxError> for Halt {
    fn from(error: SandboxError) -> Halt {
        Halt::Port(error)
    }
}

/// What woke a wait that did not halt.
enum Woke {
    /// The healthcheck attempt ended with this code.
    Job(Option<i32>),
    /// A setup service exited 0, the workload ended, or a long-running service under a fault: the
    /// caller looks again at what it waits for.
    Exit,
    Deadline,
}

impl Fleet {
    /// One batch; the moment its `create` returned.
    pub(crate) fn start(
        &mut self,
        sandbox: &impl Sandbox,
        batch: Vec<Start>,
    ) -> Result<Instant, Halt> {
        let (launches, waits): (Vec<Launch>, Vec<_>) = batch
            .into_iter()
            .map(|start| (start.launch, (start.kind, start.health)))
            .unzip();
        let ids = sandbox.create(&launches)?;
        let created = sandbox.now();
        for ((launch, (kind, health)), id) in launches.into_iter().zip(waits).zip(ids) {
            self.members.push(Member {
                kind,
                launch,
                health,
                id,
                created,
                exit: None,
                healthy: false,
            });
        }
        Ok(created)
    }

    pub(crate) fn find(&self, service: &str, replica: u32) -> Option<usize> {
        self.members
            .iter()
            .position(|member| member.launch.service == service && member.launch.replica == replica)
    }

    pub(crate) fn replicas(&self, service: &str) -> Vec<usize> {
        let indexes = self.members.iter().enumerate();
        indexes
            .filter(|(_, member)| member.launch.service == service)
            .map(|(index, _)| index)
            .collect()
    }

    /// Until the healthcheck of `index` passes once. As in Compose, an attempt goes `interval`
    /// after the end of the last, failures before `start_period` do not count, and `retries`
    /// failures in a row are `Unhealthy`; unlike Compose, the first attempt goes at once.
    pub(crate) fn healthy(
        &mut self,
        sandbox: &impl Sandbox,
        index: usize,
        deadline: Instant,
        cancel: &Cancel,
    ) -> Result<(), Halt> {
        let member = &self.members[index];
        let Some(health) = member.health.clone().filter(|_| !member.healthy) else {
            return Ok(());
        };
        let (id, counts_from) = (member.id.clone(), member.created + health.start_period);
        let mut failures = 0;
        while sandbox.now() < deadline {
            stop_if(cancel)?;
            let job = sandbox.exec_detached(&id, &health.test, health.timeout)?;
            // The guest kills the attempt at `timeout`, but a child that left its process group
            // and holds its stdout keeps the job open past the kill.
            let attempt = deadline.min(sandbox.now() + health.timeout);
            let passed = loop {
                match self.watch(sandbox, Some(&job), attempt, cancel)? {
                    Woke::Job(code) => break code == Some(0),
                    Woke::Exit => continue,
                    Woke::Deadline => break false,
                }
            };
            if passed {
                self.members[index].healthy = true;
                return Ok(());
            }
            // An attempt the deadline cut short is no failure of the service.
            if sandbox.now() >= deadline {
                break;
            }
            if sandbox.now() >= counts_from {
                failures += 1;
                if failures >= health.retries {
                    return Err(Halt::Unhealthy(index));
                }
            }
            let pause = deadline.min(sandbox.now() + health.interval);
            while !matches!(self.watch(sandbox, None, pause, cancel)?, Woke::Deadline) {}
        }
        Err(Halt::Deadline(index))
    }

    /// Until the setup service `index` exited 0; another code halts in `watch`.
    pub(crate) fn completed(
        &mut self,
        sandbox: &impl Sandbox,
        index: usize,
        deadline: Instant,
        cancel: &Cancel,
    ) -> Result<(), Halt> {
        while self.members[index].exit.is_none() {
            if let Woke::Deadline = self.watch(sandbox, None, deadline, cancel)? {
                return Err(Halt::Deadline(index));
            }
        }
        Ok(())
    }

    /// Until the workload ends: its code and `Exited.at`.
    pub(crate) fn finished(
        &mut self,
        sandbox: &impl Sandbox,
        deadline: Instant,
        cancel: &Cancel,
    ) -> Result<(Option<i32>, Instant), Halt> {
        let index = self
            .members
            .iter()
            .position(|member| member.kind == ServiceKind::Workload)
            .expect("a repeat has started its workload");
        loop {
            if let Some(exit) = self.members[index].exit {
                return Ok(exit);
            }
            if let Woke::Deadline = self.watch(sandbox, None, deadline, cancel)? {
                return Err(Halt::Deadline(index));
            }
        }
    }

    /// The first exit among the running members and `job`, or the deadline. A setup service
    /// that exits 0, the workload and a long-running service under a fault are noted for the
    /// caller; any other exit halts the phase.
    fn watch(
        &mut self,
        sandbox: &impl Sandbox,
        job: Option<&ExecId>,
        deadline: Instant,
        cancel: &Cancel,
    ) -> Result<Woke, Halt> {
        loop {
            stop_if(cancel)?;
            let now = sandbox.now();
            if now >= deadline {
                return Ok(Woke::Deadline);
            }
            let running = self.members.iter().filter(|member| member.exit.is_none());
            let mut what: Vec<Waitable> = running
                .map(|member| Waitable::Sandbox(member.id.clone()))
                .collect();
            what.extend(job.map(|job| Waitable::Exec(job.clone())));
            let Some(exited) = sandbox.wait(&what, deadline.min(now + CHUNK))? else {
                continue;
            };
            let Waitable::Sandbox(id) = &exited.what else {
                return Ok(Woke::Job(exited.code));
            };
            let index = self
                .members
                .iter()
                .position(|member| member.id == *id)
                .expect("`wait` names a member it was given");
            let member = &mut self.members[index];
            member.exit = Some((exited.code, exited.at));
            return match member.kind {
                ServiceKind::Setup if exited.code == Some(0) => Ok(Woke::Exit),
                ServiceKind::Workload => Ok(Woke::Exit),
                ServiceKind::LongRunning if self.under_fault => Ok(Woke::Exit),
                _ => Err(Halt::Down(index)),
            };
        }
    }
}

pub(crate) fn stop_if(cancel: &Cancel) -> Result<(), Halt> {
    if cancel.is_cancelled() {
        Err(Halt::Cancelled)
    } else {
        Ok(())
    }
}
