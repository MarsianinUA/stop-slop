//! What `run::run` returns (plan, row 85). The port's types come from `sandbox` without features,
//! so `report` has no copy of `Metrics`, `PortStats`, `Stopped` or `ExecOutcome`.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use camino::Utf8PathBuf;
use profile::{Coverage, Samples};
use sandbox::{ExecOutcome, Metrics, PortStats, Stopped};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug)]
pub struct Raw {
    /// From `Stand`: what the run ran.
    pub digest: String,
    pub repeat: u32,
    pub setup_timeout: Duration,
    pub timeout: Duration,
    /// Guest targets of the relative binds.
    pub harness: Vec<String>,
    /// By file name `probe.<name>.jsonl`.
    pub probes: BTreeMap<String, ProbeRule>,
    /// Bytes per volume and the `after-setup` probes; `before` only with `Versions.before`. On a
    /// cache hit they come from the entry, and `out_dir` is still in the run folder.
    pub setup: Setup,
    pub head: Vec<RepeatOutput>,
    pub before: Option<BeforeOutcome>,
    /// `None` without a profiled service, or without `Extra::Coverage` in the set: `make` then
    /// copies the pool's.
    pub coverage: Option<CoverageOutcome>,
    /// `None` without `x-stand.fault`, or without `Extra::Fault`.
    pub fault: Option<FaultOutcome>,
    /// `None` without `x-stand.second_pass`, or without `Extra::SecondPass`.
    pub second_pass: Option<SecondPassOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeRule {
    pub key: Option<String>,
    pub ignore: Vec<String>,
}

#[derive(Debug)]
pub struct Setup {
    pub head: SetupSide,
    pub before: Option<SetupSide>,
}

#[derive(Debug)]
pub struct SetupSide {
    /// By volume name.
    pub sizes: BTreeMap<String, u64>,
    pub probes: Vec<ProbeRan>,
    pub out_dir: Utf8PathBuf,
}

#[derive(Debug)]
pub struct RepeatOutput {
    /// From 1.
    pub repeat: u32,
    /// From the return of the `create` that started the workload to its `Exited.at`.
    pub wall: Duration,
    pub instances: Vec<InstanceOutput>,
    pub probes: Vec<ProbeRan>,
    /// Host folder of `/stand/out`: the workload's files and `probe.<name>.jsonl`.
    pub out_dir: Utf8PathBuf,
}

/// Long-running services and the workload; setup services do not run in repeats.
#[derive(Debug)]
pub struct InstanceOutput {
    pub service: String,
    pub replica: u32,
    pub role: Role,
    /// Read at the workload's `Exited.at`.
    pub metrics: Metrics,
    pub log: Utf8PathBuf,
    /// Bytes of `log` at the cut; error lines count only before it.
    pub log_cut: u64,
    /// `None` for the workload.
    pub stopped: Option<Stopped>,
    /// `None` for a service without `x-stand.profile`.
    pub profile: Option<ProfileOutcome>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    LongRunning,
    Workload,
}

#[derive(Debug, Clone)]
pub struct ProbeRan {
    pub name: String,
    pub outcome: ExecOutcome,
    pub out: Utf8PathBuf,
}

#[derive(Debug)]
pub enum ProfileOutcome {
    Read(Samples),
    NoProfile,
}

#[derive(Debug)]
pub enum BeforeOutcome {
    Passed(Vec<RepeatOutput>),
    Failed(Failure),
}

#[derive(Debug)]
pub enum CoverageOutcome {
    /// By service; the replicas of a service are one.
    Read(BTreeMap<String, Coverage>),
    Failed(Failure),
}

/// The fault repeats of each version (plan, row 73), in their order.
#[derive(Debug)]
pub struct FaultOutcome {
    /// `<service>:<port>`.
    pub target: String,
    /// `down`, or `slow <delay>ms`: `slow 2000ms`, `slow 1.5ms`.
    pub mode: String,
    pub head: Vec<FaultRepeat>,
    /// `None` without `Versions.before`, or when "before" failed.
    pub before: Option<Vec<FaultRepeat>>,
}

/// What a repeat under the fault ended with: its categories, never a failure of the run.
#[derive(Debug)]
pub struct FaultRepeat {
    /// From 1.
    pub repeat: u32,
    /// `None` when the workload did not end with a code: the repeat hit its timeout, ended before
    /// the workload started, or the workload was killed; `failure` tells them apart.
    pub workload: Option<i32>,
    /// Long-running services that exited during the repeat.
    pub exited: Vec<String>,
    /// What ended the repeat short of the workload's exit 0, as in a failed measured repeat. Phase
    /// `Ready`: before the fault went on, so it says nothing of the fault (a readiness that never
    /// came); `Workload`: under the fault. No probe runs in a fault repeat, so `probes` is empty.
    pub failure: Option<Failure>,
    /// At the cut; no profiler runs in a fault repeat.
    pub instances: Vec<InstanceOutput>,
}

/// The second pass of each version (plan, row 74).
#[derive(Debug)]
pub struct SecondPassOutcome {
    /// The files of `expect.unchanged`.
    pub unchanged: Vec<String>,
    pub head: PassPair,
    /// `None` without `Versions.before`, or when "before" failed.
    pub before: Option<PassPair>,
}

#[derive(Debug)]
pub enum PassPair {
    Passed {
        first: Pass,
        second: Pass,
    },
    /// `repeat` is `None`; `probes` are the `on-failure` outputs of the pass that failed.
    Failed(Failure),
}

#[derive(Debug)]
pub struct Pass {
    /// Host folder of the workload's `/stand/out` in this pass, with the `probe.<name>.jsonl` of
    /// the probes after it.
    pub out_dir: Utf8PathBuf,
    pub probes: Vec<ProbeRan>,
    /// Of every instance at the cut after this pass, the workload's included; a long-running
    /// service counts from its start, so the second pass holds the first.
    pub ports: Vec<PortStats>,
}

/// A failed lane in `Raw`, the `failure` of the `--json` line and `before.failed` of the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub phase: Phase,
    pub service: Option<String>,
    pub repeat: Option<u32>,
    pub cause: Cause,
    /// The folder of the failed setup or repeat, with its logs.
    pub logs: Option<Utf8PathBuf>,
    /// The outputs of the `on-failure` probes.
    pub probes: Vec<Utf8PathBuf>,
}

/// The steps of the plan's call order; `run` uses only `Setup`, `Ready` and `Workload`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Config,
    Spec,
    Lock,
    Seal,
    Sweep,
    Budget,
    Pull,
    Source,
    Setup,
    Ready,
    Workload,
    Report,
}

/// `Exit`: a setup service or the workload ended with a code other than 0. `Died`: a long-running
/// service ended, or a process was killed without a code. `Error`: the text of a refusal or a
/// breakage, one line, written by `cli`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cause {
    Exit(i32),
    SetupTimeout,
    Timeout,
    Died,
    Unhealthy,
    Cancelled,
    Error(String),
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Cause::Exit(code) => write!(f, "exit-{code}"),
            Cause::SetupTimeout => f.write_str("setup-timeout"),
            Cause::Timeout => f.write_str("timeout"),
            Cause::Died => f.write_str("died"),
            Cause::Unhealthy => f.write_str("unhealthy"),
            Cause::Cancelled => f.write_str("cancelled"),
            Cause::Error(text) => f.write_str(text),
        }
    }
}

impl Cause {
    fn parse(text: &str) -> Cause {
        if let Some(code) = text
            .strip_prefix("exit-")
            .and_then(|code| code.parse().ok())
        {
            return Cause::Exit(code);
        }
        match text {
            "setup-timeout" => Cause::SetupTimeout,
            "timeout" => Cause::Timeout,
            "died" => Cause::Died,
            "unhealthy" => Cause::Unhealthy,
            "cancelled" => Cause::Cancelled,
            _ => Cause::Error(text.to_owned()),
        }
    }
}

impl Serialize for Cause {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Cause {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Cause, D::Error> {
        let text = String::deserialize(deserializer)?;
        Ok(Cause::parse(&text))
    }
}

/// The repeats outside the stats: `run` runs those in its set, `Context.reused` names those
/// copied from the pool (plan, row 89).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Extra {
    Coverage,
    Fault,
    SecondPass,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cause_is_its_token_both_ways() {
        let causes = [
            (Cause::Exit(3), "exit-3"),
            (Cause::Exit(-1), "exit--1"),
            (Cause::SetupTimeout, "setup-timeout"),
            (Cause::Timeout, "timeout"),
            (Cause::Died, "died"),
            (Cause::Unhealthy, "unhealthy"),
            (Cause::Cancelled, "cancelled"),
            (
                Cause::Error("image `x` is not in the registry".into()),
                "image `x` is not in the registry",
            ),
        ];
        for (cause, token) in causes {
            let json = serde_json::to_string(&cause).unwrap();
            assert_eq!(json, serde_json::to_string(token).unwrap());
            assert_eq!(
                serde_json::from_str::<Cause>(&json).unwrap(),
                cause,
                "{token}"
            );
        }
    }

    #[test]
    fn a_failure_is_camel_case_with_a_kebab_case_phase() {
        let failure = Failure {
            phase: Phase::Workload,
            service: Some("load".into()),
            repeat: Some(2),
            cause: Cause::Timeout,
            logs: Some("runs/1/repeat-2-head".into()),
            probes: Vec::new(),
        };
        let json = serde_json::to_value(&failure).unwrap();
        assert_eq!(json["phase"], "workload");
        assert_eq!(json["cause"], "timeout");
        assert_eq!(serde_json::from_value::<Failure>(json).unwrap(), failure);
    }
}
