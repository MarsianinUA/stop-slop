//! The hot functions of the head and the coverage block. A function's key is (service, file, V8's
//! name) (plan, row 33): it outlives a shift of lines between versions, and of equal names the
//! nearest line wins.

use std::collections::{BTreeMap, BTreeSet};

use crate::Before;
use crate::flags::Flag;
use crate::raw::{CoverageOutcome, ProfileOutcome, Raw, RepeatOutput};
use crate::schema::{CalledJson, CoverageJson, Direction, Hot, HotVersus, Report, Verdict};
use crate::stats::{self, Sample};

/// Below it a function is sampling noise (Optional Assumptions: «функции с медианой self от 1 мс»).
const MIN_SELF_MS: f64 = 1.0;

/// One start of a function on one version; its times per measured repeat in which the service had a
/// profile, the replicas added up.
struct Function<'r> {
    service: &'r str,
    file: &'r str,
    name: &'r str,
    line: u32,
    column: u32,
    selfs: Vec<f64>,
    totals: Vec<f64>,
}

impl Function<'_> {
    fn sample(&self) -> Option<Sample> {
        Sample::of(&[&self.selfs])
    }
}

/// `before` and `previous` are `no-baseline` for a function they do not have; a failed "before"
/// has none.
pub(crate) fn of(raw: &Raw, before: Before<'_>, previous: Option<&Report>) -> Vec<Hot> {
    let head = functions(&raw.head);
    let before = match before {
        Before::Absent => None,
        Before::Failed(_) => Some(Vec::new()),
        Before::Passed(repeats) => Some(functions(repeats)),
    };
    let mut hot: Vec<Hot> = head
        .iter()
        .filter_map(|function| {
            let now = function.sample()?;
            if now.median < MIN_SELF_MS {
                return None;
            }
            let start = (function.line, function.column);
            let versus_before = before.as_ref().map(|functions| {
                let same = functions
                    .iter()
                    .filter(|other| same(function, other.service, other.file, other.name));
                let matched = nearest(same, start, |other| (other.line, other.column));
                versus(matched.and_then(Function::sample), now)
            });
            let last = previous.and_then(|report| {
                let same = report
                    .hot
                    .iter()
                    .filter(|other| same(function, &other.service, &other.file, &other.name));
                nearest(same, start, |other| (other.line, other.column))
            });
            let last = last.map(|other| Sample {
                median: other.self_ms,
                spread: 0.0,
            });
            Some(Hot {
                service: function.service.to_owned(),
                file: function.file.to_owned(),
                name: function.name.to_owned(),
                line: function.line,
                column: function.column,
                self_ms: stats::round(now.median, 1),
                total_ms: stats::round(stats::median(&function.totals).unwrap_or(0.0), 1),
                before: versus_before,
                previous: versus(last, now),
            })
        })
        .collect();
    hot.sort_by(|a, b| {
        let hotter = b.self_ms.total_cmp(&a.self_ms);
        hotter
            .then_with(|| a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });
    hot
}

fn same(function: &Function<'_>, service: &str, file: &str, name: &str) -> bool {
    function.service == service && function.file == file && function.name == name
}

/// The nearest line, then the nearest column, then the earlier line.
fn nearest<'c, T: 'c>(
    candidates: impl Iterator<Item = &'c T>,
    (line, column): (u32, u32),
    start: impl Fn(&T) -> (u32, u32),
) -> Option<&'c T> {
    candidates.min_by_key(|candidate| {
        let (other_line, other_column) = start(candidate);
        let lines = line.abs_diff(other_line);
        (lines, column.abs_diff(other_column), other_line)
    })
}

/// A previous report keeps only the median of a hot function, so its spread counts as 0.
fn versus(base: Option<Sample>, now: Sample) -> HotVersus {
    let verdict = base.map_or(Verdict::NoBaseline, |base| {
        stats::verdict(Direction::Lower, base, now)
    });
    HotVersus { verdict }
}

/// (service, file, line, column, name): one start of a function.
type Start<'r> = (&'r str, &'r str, u32, u32, &'r str);

fn functions(repeats: &[RepeatOutput]) -> Vec<Function<'_>> {
    let mut profiled: BTreeMap<&str, BTreeSet<usize>> = BTreeMap::new();
    // Self and total time by repeat.
    let mut times: BTreeMap<Start<'_>, BTreeMap<usize, (f64, f64)>> = BTreeMap::new();
    for (index, repeat) in repeats.iter().enumerate() {
        for instance in &repeat.instances {
            let Some(ProfileOutcome::Read(samples)) = &instance.profile else {
                continue;
            };
            let service = instance.service.as_str();
            profiled.entry(service).or_default().insert(index);
            for function in &samples.functions {
                let file = function.file.as_str();
                let key = (
                    service,
                    file,
                    function.line,
                    function.column,
                    function.name.as_str(),
                );
                let (own, total) = times.entry(key).or_default().entry(index).or_default();
                *own += function.self_ms;
                *total += function.total_ms;
            }
        }
    }
    let functions = times.into_iter().map(|(start, by_repeat)| {
        let (service, file, line, column, name) = start;
        let repeats = &profiled[service];
        let per_repeat = |part: fn(&(f64, f64)) -> f64| -> Vec<f64> {
            let times = repeats
                .iter()
                .map(|index| by_repeat.get(index).map_or(0.0, part));
            times.collect()
        };
        Function {
            service,
            file,
            name,
            line,
            column,
            selfs: per_repeat(|times| times.0),
            totals: per_repeat(|times| times.1),
        }
    });
    functions.collect()
}

/// Why the coverage repeat leaves the coverage unknown, one `coverage-unknown` each: it failed; it
/// covered no service; or `run` read a replica without chunks as one without calls, so a service
/// with no call that also wrote no profile in a measured repeat of the head may have run unseen
/// (T-12 stage 6 note). `stop-slop hot` cannot tell the services of a file apart, so any reason
/// leaves the whole block `None`, not a claim that no function of that service ran.
pub(crate) fn unknown(raw: &Raw) -> Vec<Flag> {
    let services = match &raw.coverage {
        None => return Vec::new(),
        Some(CoverageOutcome::Failed(failure)) => {
            return vec![Flag::CoverageUnknown {
                stage: Some(failure.phase),
                service: None,
            }];
        }
        Some(CoverageOutcome::Read(services)) => services,
    };
    if services.is_empty() {
        return vec![Flag::CoverageUnknown {
            stage: None,
            service: None,
        }];
    }
    let mut unseen = BTreeSet::new();
    for instance in raw.head.iter().flat_map(|repeat| &repeat.instances) {
        let uncalled = services
            .get(&instance.service)
            .is_some_and(|coverage| coverage.called.is_empty());
        if uncalled && matches!(instance.profile, Some(ProfileOutcome::NoProfile)) {
            unseen.insert(instance.service.as_str());
        }
    }
    let flags = unseen.into_iter().map(|service| Flag::CoverageUnknown {
        stage: None,
        service: Some(service.to_owned()),
    });
    flags.collect()
}

/// The functions the services of the coverage repeat entered, the services added up.
pub(crate) fn coverage(raw: &Raw) -> Option<CoverageJson> {
    let Some(CoverageOutcome::Read(services)) = &raw.coverage else {
        return None;
    };
    if !unknown(raw).is_empty() {
        return None;
    }
    let mut calls: BTreeMap<(&str, u32, u32), u64> = BTreeMap::new();
    for called in services.values().flat_map(|coverage| &coverage.called) {
        let start = (called.file.as_str(), called.line, called.column);
        let count = calls.entry(start).or_default();
        *count = count.saturating_add(called.calls);
    }
    let called = calls
        .into_iter()
        .map(|((file, line, column), calls)| CalledJson {
            file: file.to_owned(),
            line,
            column,
            calls,
        });
    Some(CoverageJson {
        services: services.keys().cloned().collect(),
        called: called.collect(),
    })
}
