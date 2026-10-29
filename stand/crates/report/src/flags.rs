//! The flags of a run (Optional Assumptions, «Флаги v1»; plan, rows 39, 60, 70, 72) and their
//! rules: facts the stand saw that the agent must not miss and that are no failure. Every flag goes
//! into the line.

use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8Path;
use profile::Samples;
use sandbox::ExecOutcome;
use serde::{Deserialize, Serialize};

use crate::outputs::Outputs;
use crate::raw::{Phase, ProfileOutcome, Raw, RepeatOutput, SetupSide};
use crate::schema::time;
use crate::stats::{self, Sample};
use crate::{Before, Context, Previous, Side, hot, sides};

/// A spread above it hides any shift under it, and most declared costs are smaller.
const HIGH_NOISE: f64 = 0.2;

/// A peak at 90% of the limit: the next run may die of it, or the kernel already reclaims.
const NEAR_LIMIT: f64 = 0.9;

/// A tenth of the TypeScript time without a source map: the hot list misses enough to mislead.
const UNMAPPED: f64 = 0.1;

/// `max_inbound` of microsandbox 0.7.8, per sandbox, so per replica (plan, row 79). Below it, drops
/// are a target that failed or was not ready yet, which its exit and readiness show.
const NETWORK_LIMIT: u64 = 256;

/// Fewer pooled samples of a version than this make the verdicts coarse.
const LOW_REPEATS: usize = 3;

/// The default `max_tolerance` of `stand.toml` (Config): a wider one lets `accept` take bounds
/// that hide what the declared cost was for.
const MAX_TOLERANCE: u16 = 25;

/// The extensions `profile` counts in `unmapped_ms`.
const TYPESCRIPT: [&str; 4] = ["ts", "tsx", "mts", "cts"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum Flag {
    HighNoise {
        measure: String,
    },
    FirstRepeatOutlier {
        measure: String,
    },
    /// The fewest pooled samples of a version.
    LowRepeats {
        samples: u32,
    },
    NoProfile {
        service: String,
    },
    Unprofiled,
    LostProcesses {
        service: String,
        count: u32,
    },
    UnmappedTime {
        service: String,
        share: f64,
    },
    MemoryNearLimit {
        service: String,
        peak: u64,
        limit: u64,
    },
    /// `drops` of both versions in that repeat.
    NetworkLimitHit {
        repeat: u32,
        target: String,
        drops: u64,
    },
    MissingExpose {
        name: String,
        attempts: u64,
    },
    /// The line's only sign that nothing was compared: the run still exits 0 (plan, row 16).
    BeforeFailed {
        stage: Phase,
    },
    /// `stage`: the coverage repeat failed there. `service`: one with no call that wrote no profile
    /// in a measured repeat either, so it may have run unseen. Both `None`: the repeat covered no
    /// service.
    CoverageUnknown {
        stage: Option<Phase>,
        service: Option<String>,
    },
    PreviousUnreadable,
    RecipeChanged,
    /// `None`: no `*.jsonl` at all.
    NoResults {
        file: Option<String>,
    },
    BadResultLine {
        file: String,
        line: u32,
    },
    DeadProbe {
        probe: String,
        reason: DeadReason,
    },
    /// `repeat` is `None` for `after-setup`, `code` for the deadline.
    ProbeFailed {
        probe: String,
        side: Side,
        repeat: Option<u32>,
        code: Option<i32>,
    },
    HeadOnlyNoise {
        file: String,
        path: String,
    },
    KilledAtStop {
        service: String,
        side: Side,
        repeat: u32,
    },
    /// The workload did not run twice on one state there: nothing of that version was compared.
    SecondPassFailed {
        side: Side,
        stage: Phase,
    },
    /// The open connections to `target` after the first pass and after the second.
    ConnectionsGrow {
        target: String,
        side: Side,
        first: u64,
        second: u64,
    },
    /// An `expect.cost` entry of a measure the report does not have.
    UnknownExpect {
        measure: String,
    },
    /// "Before" was taken after the session began, the earliest of its first run and the seal.
    BeforeAfterSession {
        taken_at: String,
        started_at: String,
    },
    /// The last passed run of this head tree before the seal: the cost was declared after its
    /// numbers.
    MeasuredBeforeSeal {
        run: String,
    },
    WideTolerance {
        max_tolerance: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeadReason {
    NoRecords,
    AllIgnored,
}

impl Flag {
    /// The order of the kinds above: a run lists its flags by kind, each kind in the order found.
    fn rank(&self) -> u8 {
        match self {
            Flag::HighNoise { .. } => 0,
            Flag::FirstRepeatOutlier { .. } => 1,
            Flag::LowRepeats { .. } => 2,
            Flag::NoProfile { .. } => 3,
            Flag::Unprofiled => 4,
            Flag::LostProcesses { .. } => 5,
            Flag::UnmappedTime { .. } => 6,
            Flag::MemoryNearLimit { .. } => 7,
            Flag::NetworkLimitHit { .. } => 8,
            Flag::MissingExpose { .. } => 9,
            Flag::BeforeFailed { .. } => 10,
            Flag::CoverageUnknown { .. } => 11,
            Flag::PreviousUnreadable => 12,
            Flag::RecipeChanged => 13,
            Flag::NoResults { .. } => 14,
            Flag::BadResultLine { .. } => 15,
            Flag::DeadProbe { .. } => 16,
            Flag::ProbeFailed { .. } => 17,
            Flag::HeadOnlyNoise { .. } => 18,
            Flag::KilledAtStop { .. } => 19,
            Flag::SecondPassFailed { .. } => 20,
            Flag::ConnectionsGrow { .. } => 21,
            Flag::UnknownExpect { .. } => 22,
            Flag::BeforeAfterSession { .. } => 23,
            Flag::MeasuredBeforeSeal { .. } => 24,
            Flag::WideTolerance { .. } => 25,
        }
    }
}

pub(crate) fn order(flags: &mut [Flag]) {
    flags.sort_by_key(Flag::rank);
}

/// What `Raw` alone shows, so a failed run has them too (plan, row 60).
pub(crate) fn facts(raw: &Raw) -> Vec<Flag> {
    let sides = sides(raw);
    let mut flags = Vec::new();
    profiles(raw, &sides, &mut flags);
    memory(&sides, &mut flags);
    network(&sides, &mut flags);
    unexposed(&sides, &mut flags);
    if let Before::Failed(failure) = Before::of(raw) {
        flags.push(Flag::BeforeFailed {
            stage: failure.phase,
        });
    }
    flags.extend(hot::unknown(raw));
    probes(&sides, &mut flags);
    stops(&sides, &mut flags);
    flags
}

type Sides<'r> = [(Side, &'r SetupSide, &'r [RepeatOutput])];

/// `unmapped-time` counts the head only: it explains the hot list, which is the head's.
fn profiles(raw: &Raw, sides: &Sides<'_>, flags: &mut Vec<Flag>) {
    let mut missing = BTreeSet::new();
    let mut lost: BTreeMap<&str, u32> = BTreeMap::new();
    let mut unmapped: BTreeMap<&str, (f64, f64)> = BTreeMap::new();
    for (side, _, repeats) in sides {
        for instance in repeats.iter().flat_map(|repeat| &repeat.instances) {
            let service = instance.service.as_str();
            match &instance.profile {
                None => {}
                Some(ProfileOutcome::NoProfile) => {
                    missing.insert(service);
                }
                Some(ProfileOutcome::Read(samples)) => {
                    *lost.entry(service).or_default() += samples.lost_processes;
                    if *side == Side::Head {
                        let (part, typescript) = unmapped.entry(service).or_default();
                        *part += samples.unmapped_ms;
                        *typescript += samples.unmapped_ms + mapped_typescript(samples);
                    }
                }
            }
        }
    }
    let missing = missing.into_iter().map(|service| Flag::NoProfile {
        service: service.to_owned(),
    });
    flags.extend(missing);
    let mut instances = raw.head.iter().flat_map(|repeat| &repeat.instances);
    if !raw.head.is_empty() && !instances.any(|instance| instance.profile.is_some()) {
        flags.push(Flag::Unprofiled);
    }
    for (service, count) in lost.into_iter().filter(|(_, count)| *count > 0) {
        flags.push(Flag::LostProcesses {
            service: service.to_owned(),
            count,
        });
    }
    for (service, (part, typescript)) in unmapped {
        let share = if typescript > 0.0 {
            part / typescript
        } else {
            0.0
        };
        if share >= UNMAPPED {
            flags.push(Flag::UnmappedTime {
                service: service.to_owned(),
                share: stats::round(share, 4),
            });
        }
    }
}

/// The TypeScript time a source map placed: unmapped time is in no function or module of
/// `Samples`, so the two add up to the TypeScript time.
fn mapped_typescript(samples: &Samples) -> f64 {
    let functions = samples.functions.iter();
    let functions = functions.map(|function| (&function.file, function.self_ms));
    let modules = samples.modules.iter();
    let modules = modules.map(|module| (&module.file, module.self_ms));
    let typescript = functions
        .chain(modules)
        .filter(|(file, _)| is_typescript(file));
    typescript.map(|(_, ms)| ms).sum()
}

/// As `profile` tells it: a file named `.ts` has no extension.
fn is_typescript(file: &Utf8Path) -> bool {
    file.extension()
        .is_some_and(|extension| TYPESCRIPT.contains(&extension))
}

/// The instance and repeat closest to its limit, of each service; the workload too, whose memory
/// can slow the run it drives.
fn memory(sides: &Sides<'_>, flags: &mut Vec<Flag>) {
    let mut closest: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    let ratio = |(peak, limit): (u64, u64)| {
        if limit == 0 {
            0.0
        } else {
            peak as f64 / limit as f64
        }
    };
    for (_, _, repeats) in sides {
        for instance in repeats.iter().flat_map(|repeat| &repeat.instances) {
            let seen = (instance.metrics.memory_peak, instance.metrics.memory_limit);
            let entry = closest.entry(instance.service.as_str()).or_insert(seen);
            if ratio(seen) > ratio(*entry) {
                *entry = seen;
            }
        }
    }
    for (service, (peak, limit)) in closest {
        if ratio((peak, limit)) >= NEAR_LIMIT {
            flags.push(Flag::MemoryNearLimit {
                service: service.to_owned(),
                peak,
                limit,
            });
        }
    }
}

/// The counters of a target add up over the guests of a repeat; the limit counts the connections
/// the target took in that repeat of one version, whose drops alone add to the flag (plan T-12,
/// orchestrator's note to T-11 stage 2). `PortStats` names no replica, so a target with `r`
/// replicas needs `256 r`: from there up, at least one of them took 256. `open` plays no part: a
/// relay closes up to half a second after its client.
fn network(sides: &Sides<'_>, flags: &mut Vec<Flag>) {
    let mut hits: BTreeMap<(u32, &str), u64> = BTreeMap::new();
    for (_, _, repeats) in sides {
        for repeat in *repeats {
            let mut targets: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
            let instances = repeat.instances.iter();
            for port in instances.flat_map(|instance| &instance.metrics.ports) {
                let (drops, connections) = targets.entry(port.target.as_str()).or_default();
                *drops += port.drops;
                *connections += port.connections;
            }
            for (target, (drops, connections)) in targets {
                let limit = NETWORK_LIMIT.saturating_mul(replicas(repeat, target));
                if drops >= 1 && connections >= limit {
                    *hits.entry((repeat.repeat, target)).or_default() += drops;
                }
            }
        }
    }
    for ((repeat, target), drops) in hits {
        flags.push(Flag::NetworkLimitHit {
            repeat,
            target: target.to_owned(),
            drops,
        });
    }
}

/// The instances of the service behind `<service>:<port>` in this repeat, one per replica; at
/// least 1.
fn replicas(repeat: &RepeatOutput, target: &str) -> u64 {
    let service = target
        .rsplit_once(':')
        .map_or(target, |(service, _)| service);
    let instances = repeat.instances.iter();
    let count = instances
        .filter(|instance| instance.service == service)
        .count();
    u64::try_from(count).unwrap_or(u64::MAX).max(1)
}

fn unexposed(sides: &Sides<'_>, flags: &mut Vec<Flag>) {
    let mut names: BTreeMap<&str, u64> = BTreeMap::new();
    for (_, _, repeats) in sides {
        let instances = repeats.iter().flat_map(|repeat| &repeat.instances);
        for call in instances.flat_map(|instance| &instance.metrics.unexposed) {
            *names.entry(call.name.as_str()).or_default() += call.attempts;
        }
    }
    // `net` counts every peer name without `expose`, so most come with 0: a name nobody called.
    for (name, attempts) in names.into_iter().filter(|&(_, attempts)| attempts > 0) {
        flags.push(Flag::MissingExpose {
            name: name.to_owned(),
            attempts,
        });
    }
}

/// A code other than 0 or the deadline is what the probe saw, so a flag and not a failure.
fn probes(sides: &Sides<'_>, flags: &mut Vec<Flag>) {
    for (side, setup, repeats) in sides {
        let after_setup = setup.probes.iter().map(|probe| (None, probe));
        let after_workload = repeats.iter().flat_map(|repeat| {
            let probes = repeat.probes.iter();
            probes.map(|probe| (Some(repeat.repeat), probe))
        });
        for (repeat, probe) in after_setup.chain(after_workload) {
            let code = match probe.outcome {
                ExecOutcome::Exited(0) => continue,
                ExecOutcome::Exited(code) => Some(code),
                ExecOutcome::TimedOut => None,
            };
            flags.push(Flag::ProbeFailed {
                probe: probe.name.clone(),
                side: *side,
                repeat,
                code,
            });
        }
    }
}

/// The measured repeats only: `Raw.fault` is never read here, where a fault may hold the stop
/// (plan, row 72).
fn stops(sides: &Sides<'_>, flags: &mut Vec<Flag>) {
    let mut killed = BTreeSet::new();
    for (side, _, repeats) in sides {
        for repeat in *repeats {
            for instance in &repeat.instances {
                if instance.stopped.is_some_and(|stopped| stopped.killed) {
                    killed.insert((*side, repeat.repeat, instance.service.as_str()));
                }
            }
        }
    }
    for (side, repeat, service) in killed {
        flags.push(Flag::KilledAtStop {
            service: service.to_owned(),
            side,
            repeat,
        });
    }
}

pub(crate) fn of_context(raw: &Raw, context: &Context<'_>) -> Vec<Flag> {
    let mut flags = match context.previous {
        Previous::None => Vec::new(),
        Previous::Unreadable => vec![Flag::PreviousUnreadable],
        Previous::Read(previous) if previous.recipe.digest != raw.digest => {
            vec![Flag::RecipeChanged]
        }
        Previous::Read(_) => Vec::new(),
    };
    if context.before.is_some() {
        seal(context, &mut flags);
    }
    flags
}

/// The facts of a comparison (plan, row 65): none of them blocks it.
fn seal(context: &Context<'_>, flags: &mut Vec<Flag>) {
    let seal = context.seal.as_ref();
    let earlier = context.history.iter().map(|run| run.created_at);
    let started = earlier
        .chain([context.created_at])
        .chain(seal.map(|seal| seal.sealed_at))
        .min();
    if let (Some(before), Some(started)) = (&context.before, started)
        && before.taken_at > started
    {
        flags.push(Flag::BeforeAfterSession {
            taken_at: time(before.taken_at),
            started_at: time(started),
        });
    }
    let Some(seal) = seal else {
        return;
    };
    let measured = context.history.iter().rev().find(|run| {
        run.passed && run.head_tree == context.head_tree && run.created_at < seal.sealed_at
    });
    if let Some(measured) = measured {
        flags.push(Flag::MeasuredBeforeSeal {
            run: measured.run.clone(),
        });
    }
    if seal.max_tolerance > MAX_TOLERANCE {
        flags.push(Flag::WideTolerance {
            max_tolerance: seal.max_tolerance,
        });
    }
}

pub(crate) fn bad_lines(outputs: &Outputs) -> Vec<Flag> {
    let lines = outputs.bad_lines.iter();
    let flags = lines.map(|(file, line)| Flag::BadResultLine {
        file: file.clone(),
        line: *line,
    });
    flags.collect()
}

/// `before` is the fewest of a version that was compared: a failed "before" has none.
pub(crate) fn low_repeats(head: usize, before: Option<usize>) -> Option<Flag> {
    let fewest = before.map_or(head, |before| before.min(head));
    let samples = u32::try_from(fewest).unwrap_or(u32::MAX);
    (fewest < LOW_REPEATS).then_some(Flag::LowRepeats { samples })
}

/// By the spread rounded as the report prints it: a printed `0.2` is not above 20%.
pub(crate) fn high_noise(measure: &str, head: Sample, before: Option<Sample>) -> Option<Flag> {
    let wide = |sample: Sample| stats::round(sample.spread, 4) > HIGH_NOISE;
    let noisy = wide(head) || before.is_some_and(wide);
    noisy.then(|| Flag::HighNoise {
        measure: measure.to_owned(),
    })
}

/// The first repeat of this run lies outside the others by more than their threshold: a warm-up
/// the median sets aside, which the agent should still know about.
pub(crate) fn first_repeat_outlier(samples: &[f64]) -> bool {
    let [first, rest @ ..] = samples else {
        return false;
    };
    if rest.len() < 2 {
        return false;
    }
    let low = rest.iter().copied().fold(f64::INFINITY, f64::min);
    let high = rest.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let middle = stats::median(rest).unwrap_or(0.0).abs();
    let margin = stats::MIN_SHIFT.max(stats::range(rest)) * middle;
    *first > high + margin || *first < low - margin
}
