//! The repeats outside the stats (plan, rows 73, 74): the categories of the fault repeats, the
//! second pass with its open connections.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use sandbox::PortStats;

use crate::flags::Flag;
use crate::outputs::Outputs;
use crate::raw::{FaultRepeat, PassPair, Phase, Raw, Role};
use crate::schema::{
    Categories, CategoryChange, CategoryJson, FaultJson, PassFile, PassOpen, PassState,
    SecondPassJson,
};
use crate::{Context, Side, behaviour};

/// The value of a category in a repeat that ended before the fault went on.
const UNKNOWN: &str = "unknown";

/// The HTTP classes of `PortStats.http`, 1xx to 5xx.
const CLASSES: [&str; 5] = ["1xx", "2xx", "3xx", "4xx", "5xx"];

/// What a fault repeat ended with (plan, row 73), in the order the report lists them.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Category {
    /// The workload's exit, or the cause it did not end with one.
    Workload,
    /// `running` or `exited`.
    State(String),
    /// The connections to the target, in the buckets of Config.
    Attempts(String),
    /// The HTTP classes the target answered with, `none` without an answer.
    Responses(String),
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Category::Workload => f.write_str("workload"),
            Category::State(service) => write!(f, "{service} state"),
            Category::Attempts(target) => write!(f, "{target} attempts"),
            Category::Responses(target) => write!(f, "{target} responses"),
        }
    }
}

type Values = BTreeMap<Category, String>;

pub(crate) fn fault(raw: &Raw) -> Option<FaultJson> {
    let outcome = raw.fault.as_ref()?;
    let head: Vec<Option<Values>> = outcome.head.iter().map(categories).collect();
    let before = outcome.before.as_ref();
    let before: Option<Vec<Option<Values>>> =
        before.map(|repeats| repeats.iter().map(categories).collect());
    let mut names = BTreeSet::from([Category::Workload]);
    let seen = head.iter().chain(before.iter().flatten()).flatten();
    names.extend(seen.flat_map(|values| values.keys().cloned()));
    let column = |repeats: &[Option<Values>]| -> Vec<CategoryJson> {
        let columns = names.iter().map(|name| CategoryJson {
            category: name.to_string(),
            values: repeats
                .iter()
                .map(|values| value(values.as_ref(), name))
                .collect(),
        });
        columns.collect()
    };
    let categories = Categories {
        before: before.as_deref().map(column),
        head: column(&head),
    };
    let changed = changed(&categories);
    Some(FaultJson {
        target: outcome.target.clone(),
        mode: outcome.mode.clone(),
        categories,
        changed,
    })
}

/// `None` for a repeat that ended before the fault went on: a readiness that never came says
/// nothing of the fault (batch review 5).
fn categories(repeat: &FaultRepeat) -> Option<Values> {
    let failure = repeat.failure.as_ref();
    if failure.is_some_and(|failure| failure.phase == Phase::Ready) {
        return None;
    }
    let workload = match (repeat.workload, failure) {
        (Some(code), _) => format!("exit-{code}"),
        (None, Some(failure)) => failure.cause.to_string(),
        (None, None) => return None,
    };
    let mut values = Values::from([(Category::Workload, workload)]);
    let services = repeat.instances.iter();
    let services = services.filter(|instance| instance.role == Role::LongRunning);
    for instance in services {
        let exited = repeat.exited.contains(&instance.service);
        let state = if exited { "exited" } else { "running" };
        values.insert(Category::State(instance.service.clone()), state.to_owned());
    }
    let ports = repeat.instances.iter();
    let ports: Vec<&PortStats> = ports.flat_map(|instance| &instance.metrics.ports).collect();
    let mut attempts: BTreeMap<&str, u64> = BTreeMap::new();
    let mut answers: BTreeMap<&str, [u64; 5]> = BTreeMap::new();
    for port in ports {
        *attempts.entry(&port.target).or_default() += port.connections;
        if let Some(classes) = port.http {
            let added = answers.entry(&port.target).or_default();
            for (sum, count) in added.iter_mut().zip(classes) {
                *sum += count;
            }
        }
    }
    for (target, count) in attempts {
        values.insert(Category::Attempts(target.to_owned()), bucket(count));
    }
    for (target, classes) in answers {
        let seen = CLASSES.iter().zip(classes).filter(|(_, count)| *count > 0);
        let seen: Vec<&str> = seen.map(|(class, _)| *class).collect();
        let text = if seen.is_empty() {
            "none".to_owned()
        } else {
            seen.join(" ")
        };
        values.insert(Category::Responses(target.to_owned()), text);
    }
    Some(values)
}

/// Config: 0, 1, 2-3, 4-10, more than 10. A retry without a pause shows as the last.
fn bucket(count: u64) -> String {
    let bucket = match count {
        0 | 1 => return count.to_string(),
        2..=3 => "2-3",
        4..=10 => "4-10",
        _ => ">10",
    };
    bucket.to_owned()
}

/// A category a repeat that reached the fault did not see: no connection, no answer.
fn value(values: Option<&Values>, name: &Category) -> String {
    let Some(values) = values else {
        return UNKNOWN.to_owned();
    };
    match (values.get(name), name) {
        (Some(value), _) => value.clone(),
        (None, Category::Attempts(_)) => bucket(0),
        (None, Category::Responses(_)) => "none".to_owned(),
        (None, Category::Workload | Category::State(_)) => UNKNOWN.to_owned(),
    }
}

/// The one value all repeats of "before" agree on; repeats that disagree, or one that ended before
/// the fault went on, leave the category unknown rather than changed (plan, row 89).
fn agreed(category: &CategoryJson) -> Option<&str> {
    let first = category.values.first()?;
    let one = category.values.iter().all(|value| value == first);
    (one && first != UNKNOWN).then_some(first.as_str())
}

/// The values the head showed, each once, in the order of its repeats.
fn shown(category: &CategoryJson) -> Vec<&str> {
    let mut shown: Vec<&str> = Vec::new();
    for value in &category.values {
        if value != UNKNOWN && !shown.contains(&value.as_str()) {
            shown.push(value);
        }
    }
    shown
}

fn changed(categories: &Categories) -> Vec<CategoryChange> {
    let Some(before) = &categories.before else {
        return Vec::new();
    };
    let mut changed = Vec::new();
    for (head, before) in categories.head.iter().zip(before) {
        let Some(was) = agreed(before) else {
            continue;
        };
        for now in shown(head).into_iter().filter(|now| *now != was) {
            changed.push(CategoryChange {
                category: head.category.clone(),
                before: was.to_owned(),
                head: now.to_owned(),
            });
        }
    }
    changed
}

impl FaultJson {
    /// A category the repeats of "before" disagreed on, or one no repeat of the head reached the
    /// fault for: a pooled rerun runs the fault repeats again then (plan, row 89).
    pub fn unknown(&self) -> bool {
        !self.unknown_categories().is_empty()
    }

    pub(crate) fn unknown_categories(&self) -> Vec<String> {
        let before = self.categories.before.as_ref();
        let unknown = self
            .categories
            .head
            .iter()
            .enumerate()
            .filter(|(index, head)| {
                let undecided = before
                    .and_then(|before| before.get(*index))
                    .is_some_and(|before| agreed(before).is_none());
                shown(head).is_empty() || undecided
            });
        unknown.map(|(_, head)| head.category.clone()).collect()
    }
}

/// `None` without a second pass of the head; a failed pass of either version is a flag.
pub(crate) fn second_pass(
    raw: &Raw,
    outputs: &Outputs,
    context: &Context<'_>,
) -> (Option<SecondPassJson>, Vec<Flag>) {
    let Some(outcome) = &raw.second_pass else {
        return (None, Vec::new());
    };
    let pairs = [
        (Side::Head, Some(&outcome.head)),
        (Side::Before, outcome.before.as_ref()),
    ];
    let mut flags = Vec::new();
    let mut open = Vec::new();
    for (side, pair) in pairs {
        match pair {
            None => {}
            Some(PassPair::Failed(failure)) => flags.push(Flag::SecondPassFailed {
                side: side.as_str().to_owned(),
                stage: failure.phase,
            }),
            Some(PassPair::Passed { first, second }) => {
                let (first, second) = (open_by_target(&first.ports), open_by_target(&second.ports));
                let targets: BTreeSet<&str> = first.keys().chain(second.keys()).copied().collect();
                open.extend(targets.into_iter().map(|target| PassOpen {
                    target: target.to_owned(),
                    side: side.as_str().to_owned(),
                    first: first.get(target).copied().unwrap_or(0),
                    second: second.get(target).copied().unwrap_or(0),
                }));
            }
        }
    }
    if !matches!(outcome.head, PassPair::Passed { .. }) {
        return (None, flags);
    }
    let state = |file: &str, side: Side| {
        let records = outputs.passes.get(file)?.get(&side)?;
        Some(behaviour::passes(
            raw,
            outputs,
            context,
            (file, side),
            records,
        ))
    };
    let files = outcome.unchanged.iter().map(|file| {
        let diffs = state(file, Side::Head).unwrap_or_default();
        PassFile {
            file: file.clone(),
            before: state(file, Side::Before).map(|diffs| changed_if(!diffs.is_empty())),
            head: changed_if(!diffs.is_empty()),
            diffs,
        }
    });
    let json = SecondPassJson {
        files: files.collect(),
        open,
    };
    (Some(json), flags)
}

fn changed_if(changed: bool) -> PassState {
    if changed {
        PassState::Changed
    } else {
        PassState::Same
    }
}

fn open_by_target(ports: &[PortStats]) -> BTreeMap<&str, u64> {
    let mut open: BTreeMap<&str, u64> = BTreeMap::new();
    for port in ports {
        *open.entry(&port.target).or_default() += port.open;
    }
    open
}

/// Open connections that grew from one pass to the next: a pool or a client that never closes
/// (plan, row 74).
pub(crate) fn grown(second_pass: Option<&SecondPassJson>) -> Vec<Flag> {
    let open = second_pass.into_iter().flat_map(|pass| &pass.open);
    let grown = open.filter(|open| open.second > open.first);
    let flags = grown.map(|open| Flag::ConnectionsGrow {
        target: open.target.clone(),
        side: open.side.clone(),
        first: open.first,
        second: open.second,
    });
    flags.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempts_fall_into_the_buckets_of_config() {
        let buckets = [0, 1, 2, 3, 4, 10, 11, 500].map(bucket);
        assert_eq!(
            buckets,
            ["0", "1", "2-3", "2-3", "4-10", "4-10", ">10", ">10"]
        );
    }
}
