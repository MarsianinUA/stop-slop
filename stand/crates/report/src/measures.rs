//! The measures of a run (architecture plan, rows 22, 69, 72, 84): `<subject>.<metric>`, the samples of
//! each version pooled with the runs of the pool (row 68), and the verdicts against "before" and the
//! previous run, and the grades against the declared cost.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::Duration;

use crate::flags::{self, Flag};
use crate::grade;
use crate::outputs::Outputs;
use crate::raw::{ProfileOutcome, Raw, RepeatOutput, Role};
use crate::schema::{Direction, Measure, Src, Unit, Verdict, VersusBefore, VersusPrevious};
use crate::stats::{self, Sample};
use crate::{Before, Context, Side};

/// In the order the measures of one subject print.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Metric {
    Wall,
    HotOwned,
    HotCollateral,
    Cpu,
    Memory,
    ErrorLines,
    Shutdown,
    Connections,
    Bytes,
    Http5xxShare,
}

impl Metric {
    fn as_str(self) -> &'static str {
        match self {
            Metric::Wall => "wall",
            Metric::HotOwned => "hot-owned",
            Metric::HotCollateral => "hot-collateral",
            Metric::Cpu => "cpu",
            Metric::Memory => "memory",
            Metric::ErrorLines => "error-lines",
            Metric::Shutdown => "shutdown",
            Metric::Connections => "connections",
            Metric::Bytes => "bytes",
            Metric::Http5xxShare => "http-5xx-share",
        }
    }

    fn unit(self) -> Unit {
        match self {
            Metric::Wall
            | Metric::HotOwned
            | Metric::HotCollateral
            | Metric::Cpu
            | Metric::Shutdown => Unit::Ms,
            Metric::Memory | Metric::Bytes => Unit::Bytes,
            Metric::ErrorLines | Metric::Connections => Unit::Count,
            Metric::Http5xxShare => Unit::Share,
        }
    }

    /// Connections and bytes have no better side (Optional Assumptions, «Направление»).
    fn direction(self) -> Direction {
        match self {
            Metric::Connections | Metric::Bytes => Direction::Neutral,
            _ => Direction::Lower,
        }
    }
}

/// The subjects of row 84 in the order the report lists them: `stand`, a service, a port
/// `<service>:<port>`, a volume `volume:<name>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Stand(Metric),
    Service(String, Metric),
    Port(String, Metric),
    Volume(String),
}

impl Key {
    fn metric(&self) -> Metric {
        match self {
            Key::Stand(metric) | Key::Service(_, metric) | Key::Port(_, metric) => *metric,
            Key::Volume(_) => Metric::Bytes,
        }
    }

    /// From the profiles: a repeat without one has no sample, not a 0.
    fn profiled(&self) -> bool {
        matches!(self, Key::Stand(Metric::HotOwned | Metric::HotCollateral))
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Stand(metric) => write!(f, "stand.{}", metric.as_str()),
            Key::Service(subject, metric) | Key::Port(subject, metric) => {
                write!(f, "{subject}.{}", metric.as_str())
            }
            Key::Volume(name) => write!(f, "volume:{name}.bytes"),
        }
    }
}

pub(crate) fn of(
    raw: &Raw,
    outputs: &Outputs,
    context: &Context<'_>,
    before: Before<'_>,
) -> (Vec<Measure>, Vec<Flag>) {
    // Without `--before` nothing changed: the own cost needs a version to differ from.
    let changed = context.before.as_ref().and(context.changed.as_ref());
    let of_repeats = |repeats: &[RepeatOutput]| -> Vec<BTreeMap<Key, f64>> {
        repeats
            .iter()
            .map(|repeat| values(repeat, outputs, changed))
            .collect()
    };
    let head = of_repeats(&raw.head);
    let passed = match before {
        Before::Passed(repeats) => Some(of_repeats(repeats)),
        Before::Absent | Before::Failed(_) => None,
    };
    let repeats = head.iter().chain(passed.iter().flatten());
    let mut keys: BTreeSet<Key> = repeats.flat_map(|repeat| repeat.keys().cloned()).collect();
    let before_sizes = passed
        .as_ref()
        .and(raw.setup.before.as_ref())
        .map(|setup| &setup.sizes);
    let before_volumes = before_sizes.into_iter().flat_map(|sizes| sizes.keys());
    let volumes = raw.setup.head.sizes.keys().chain(before_volumes);
    keys.extend(volumes.map(|name| Key::Volume(name.clone())));

    let mut measures = Vec::with_capacity(keys.len());
    let mut flags = Vec::new();
    let mut counts = None;
    for key in keys {
        let (own_head, own_before) = match &key {
            Key::Volume(name) => (
                size(&raw.setup.head.sizes, name),
                before_sizes.map(|sizes| size(sizes, name)),
            ),
            _ => (
                column(&head, &key),
                passed.as_ref().map(|repeats| column(repeats, &key)),
            ),
        };
        let name = key.to_string();
        let setup = matches!(key, Key::Volume(_));
        let own_before = own_before.as_deref();
        let attempts = Attempts::of(&name, setup, &own_head, own_before, context);
        if key == Key::Stand(Metric::Wall) {
            counts = Some(attempts.counts());
        }
        let (measure, noisy) = measure(
            &key, name, &own_head, own_before, &attempts, before, context,
        );
        flags.extend(noisy);
        measures.push(measure);
    }
    if let Some((head, before)) = counts {
        flags.extend(flags::low_repeats(head, before));
    }
    (measures, flags)
}

fn measure(
    key: &Key,
    name: String,
    own_head: &[f64],
    own_before: Option<&[f64]>,
    attempts: &Attempts<'_>,
    before: Before<'_>,
    context: &Context<'_>,
) -> (Measure, Vec<Flag>) {
    let metric = key.metric();
    let direction = metric.direction();
    let head = Sample::of(&attempts.head).unwrap_or(Sample {
        median: 0.0,
        spread: 0.0,
    });
    let base = Sample::of(&attempts.before);
    let mut flags = Vec::new();
    flags.extend(flags::high_noise(&name, head, base));
    if flags::first_repeat_outlier(own_head) || own_before.is_some_and(flags::first_repeat_outlier)
    {
        flags.push(Flag::FirstRepeatOutlier {
            measure: name.clone(),
        });
    }
    let samples = own_before.map(<[f64]>::to_vec).unwrap_or_default();
    let mut expect = None;
    let versus_before = match (before, base) {
        (Before::Absent, _) => None,
        (Before::Passed(_), Some(base)) => {
            let verdict = stats::verdict(direction, base, head);
            expect = Some(grade::expect(&name, verdict, base, head, context));
            Some(VersusBefore {
                samples,
                median: Some(base.median),
                verdict,
            })
        }
        (Before::Failed(_) | Before::Passed(_), _) => Some(VersusBefore {
            samples,
            median: None,
            verdict: Verdict::NoBaseline,
        }),
    };
    let previous = context.previous.report();
    let last = previous.and_then(|report| find(&report.measures, &name));
    let versus_previous = match last {
        Some(last) => {
            let base = Sample {
                median: last.median,
                spread: last.spread,
            };
            VersusPrevious {
                median: Some(last.median),
                verdict: stats::verdict(direction, base, head),
            }
        }
        None => VersusPrevious {
            median: None,
            verdict: Verdict::NoBaseline,
        },
    };
    let measure = Measure {
        name,
        unit: metric.unit(),
        direction,
        src: Src::Stand,
        samples: own_head.to_vec(),
        median: head.median,
        spread: stats::round(head.spread, 4),
        before: versus_before,
        previous: versus_previous,
        expect,
    };
    (measure, flags)
}

/// The values of one measured repeat. The instances of a service add up their CPU and counters and
/// keep the largest memory and stop (Optional Assumptions); the workload is harness, so its own VM
/// gives no measure, only its connections at their targets.
fn values(
    repeat: &RepeatOutput,
    outputs: &Outputs,
    changed: Option<&BTreeSet<String>>,
) -> BTreeMap<Key, f64> {
    let mut values = BTreeMap::new();
    values.insert(Key::Stand(Metric::Wall), millis(repeat.wall));
    if let Some(changed) = changed {
        cost(repeat, changed, &mut values);
    }
    let mut http: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    for instance in &repeat.instances {
        if instance.role == Role::LongRunning {
            let at = |metric| Key::Service(instance.service.clone(), metric);
            *values.entry(at(Metric::Cpu)).or_default() += millis(instance.metrics.cpu);
            let memory = values.entry(at(Metric::Memory)).or_default();
            *memory = memory.max(instance.metrics.memory_peak as f64);
            let errors = outputs.errors(&instance.log);
            *values.entry(at(Metric::ErrorLines)).or_default() += f64::from(errors);
            if let Some(stopped) = instance.stopped {
                let shutdown = values.entry(at(Metric::Shutdown)).or_default();
                *shutdown = shutdown.max(millis(stopped.after));
            }
        }
        for port in &instance.metrics.ports {
            let at = |metric| Key::Port(port.target.clone(), metric);
            *values.entry(at(Metric::Connections)).or_default() += port.connections as f64;
            let bytes = port.bytes_out + port.bytes_in;
            *values.entry(at(Metric::Bytes)).or_default() += bytes as f64;
            if let Some(classes) = port.http {
                let (all, failed) = http.entry(port.target.as_str()).or_default();
                *all += classes.iter().sum::<u64>();
                *failed += classes[4];
            }
        }
    }
    for (target, (all, failed)) in http {
        let share = if all == 0 {
            0.0
        } else {
            failed as f64 / all as f64
        };
        let key = Key::Port(target.to_owned(), Metric::Http5xxShare);
        values.insert(key, stats::round(share, 4));
    }
    values
}

/// The self time of the functions in the changed files, and of the others (plan, row 69), over
/// the instances with a profile.
fn cost(repeat: &RepeatOutput, changed: &BTreeSet<String>, values: &mut BTreeMap<Key, f64>) {
    let mut split = None;
    for instance in &repeat.instances {
        let Some(ProfileOutcome::Read(samples)) = &instance.profile else {
            continue;
        };
        let (owned, collateral) = split.get_or_insert((0.0, 0.0));
        for function in &samples.functions {
            if changed.contains(function.file.as_str()) {
                *owned += function.self_ms;
            } else {
                *collateral += function.self_ms;
            }
        }
    }
    if let Some((owned, collateral)) = split {
        values.insert(Key::Stand(Metric::HotOwned), stats::round(owned, 1));
        values.insert(
            Key::Stand(Metric::HotCollateral),
            stats::round(collateral, 1),
        );
    }
}

/// One sample per repeat; a repeat without the measure, such as a target it never called, gives 0.
fn column(repeats: &[BTreeMap<Key, f64>], key: &Key) -> Vec<f64> {
    let samples = repeats.iter().map(|repeat| repeat.get(key).copied());
    if key.profiled() {
        return samples.flatten().collect();
    }
    samples.map(|sample| sample.unwrap_or(0.0)).collect()
}

fn size(sizes: &BTreeMap<String, u64>, name: &str) -> Vec<f64> {
    sizes
        .get(name)
        .map(|bytes| vec![*bytes as f64])
        .unwrap_or_default()
}

fn find<'r>(measures: &'r [Measure], name: &str) -> Option<&'r Measure> {
    measures.iter().find(|measure| measure.name == name)
}

fn millis(duration: Duration) -> f64 {
    duration.as_millis() as f64
}

/// The own samples of each run of the pool and of this run, oldest first (plan, row 68). A setup
/// size is one measurement of its setup: runs with one setup origin give one attempt (row 88).
struct Attempts<'a> {
    head: Vec<&'a [f64]>,
    before: Vec<&'a [f64]>,
}

impl<'a> Attempts<'a> {
    fn of(
        name: &str,
        setup: bool,
        own_head: &'a [f64],
        own_before: Option<&'a [f64]>,
        context: &'a Context<'_>,
    ) -> Attempts<'a> {
        Attempts {
            head: attempts(name, Side::Head, setup, Some(own_head), context),
            before: attempts(name, Side::Before, setup, own_before, context),
        }
    }

    /// The pooled samples of each version, for `low-repeats`.
    fn counts(&self) -> (usize, Option<usize>) {
        let count =
            |attempts: &[&[f64]]| attempts.iter().map(|attempt| attempt.len()).sum::<usize>();
        let before = (!self.before.is_empty()).then(|| count(&self.before));
        (count(&self.head), before)
    }
}

/// The attempts of one version that have samples; with `setup`, only the first of each origin.
fn attempts<'a>(
    name: &str,
    side: Side,
    setup: bool,
    own: Option<&'a [f64]>,
    context: &'a Context<'_>,
) -> Vec<&'a [f64]> {
    let pooled = context.pool.iter().filter_map(|report| {
        let measure = find(&report.measures, name)?;
        let samples = match side {
            Side::Head => &measure.samples,
            Side::Before => &measure.before.as_ref()?.samples,
        };
        Some((report.origin(side)?, samples.as_slice()))
    });
    let own = own.map(|own| (context.origin(side), own));
    let mut origins = BTreeSet::new();
    let kept = pooled
        .chain(own)
        .filter(|(from, samples)| !samples.is_empty() && (!setup || origins.insert(*from)));
    kept.map(|(_, samples)| samples).collect()
}
