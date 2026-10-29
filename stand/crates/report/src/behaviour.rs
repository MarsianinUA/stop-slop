//! The behaviour of the two versions (architecture plan, rows 16, 55, 62, 70): each `*.jsonl` of
//! `/stand/out`, the probes' outputs among them, "before" against the head, record by record or by
//! the probe's `key`, leaf by leaf along JSON Pointers. What differs between repeats of "before" is
//! noise; what differs only between repeats of the head is a change and stays in the diff.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::Value;

use crate::flags::{DeadReason, Flag};
use crate::outputs::{FileRecords, Outputs, SETUP};
use crate::raw::{ProbeRule, Raw};
use crate::schema::{At, Behaviour, Diff, Src};
use crate::{Context, Side, sides};

static NULL: Value = Value::Null;

/// How the records of one file compare: by position, or by the value at `key`; `ignore` takes
/// paths out.
#[derive(Debug, Clone, Copy, Default)]
struct Rule<'r> {
    key: Option<&'r str>,
    ignore: &'r [String],
}

impl<'r> Rule<'r> {
    fn of(probe: Option<&'r ProbeRule>) -> Rule<'r> {
        probe.map_or_else(Rule::default, |probe| Rule {
            key: probe.key.as_deref(),
            ignore: &probe.ignore,
        })
    }

    /// The path, or a path below it, is in `ignore`.
    fn ignores(&self, path: &str) -> bool {
        self.ignore.iter().any(|ignored| {
            let below = path.strip_prefix(ignored.as_str());
            below.is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        })
    }

    /// Whether `ignore` leaves any field of `value` (plan, row 70: a probe whose `ignore` takes
    /// every field of every record is dead).
    fn keeps(&self, path: &mut String, value: &Value) -> bool {
        let child = |path: &mut String, token: &str, value: &Value| {
            let len = path.len();
            push_token(path, token);
            let kept = self.keeps(path, value);
            path.truncate(len);
            kept
        };
        match value {
            Value::Object(fields) => fields.iter().any(|(key, value)| child(path, key, value)),
            Value::Array(items) => {
                let mut items = items.iter().enumerate();
                items.any(|(index, value)| child(path, &index.to_string(), value))
            }
            _ => !self.ignores(path),
        }
    }
}

/// Every file of this run, with `--before` that passed.
pub(crate) fn of(
    raw: &Raw,
    outputs: &Outputs,
    context: &Context<'_>,
) -> (Vec<Behaviour>, Vec<Flag>) {
    let mut files = Vec::new();
    let mut flags = Vec::new();
    if outputs.records.files.is_empty() {
        flags.push(Flag::NoResults { file: None });
    }
    for (name, this) in &outputs.records.files {
        let probe = raw.probes.get(name);
        let rule = Rule::of(probe);
        if this.head.is_empty() || this.before.is_empty() {
            flags.push(Flag::NoResults {
                file: Some(name.clone()),
            });
        }
        let before = lists(name, Side::Before, outputs, context, this);
        let head = lists(name, Side::Head, outputs, context, this);
        let noise = changing(rule, &before);
        let head_only: Vec<String> = if before.len() >= 2 && head.len() >= 2 {
            let changes = changing(rule, &head);
            changes.difference(&noise).cloned().collect()
        } else {
            Vec::new()
        };
        // The last repeat of each version: its end state is what `inspect` restores.
        let last = |side| {
            let last = this.side(side).values().next_back();
            last.map_or(&[][..], Vec::as_slice)
        };
        let mut diffs = diff(rule, last(Side::Before), last(Side::Head));
        diffs.retain(|diff| !noise.contains(&diff.path));
        let found = head_only.iter().map(|path| Flag::HeadOnlyNoise {
            file: name.clone(),
            path: path.clone(),
        });
        flags.extend(found);
        files.push(Behaviour {
            file: name.clone(),
            src: if probe.is_some() {
                Src::Probe
            } else {
                Src::Workload
            },
            key: rule.key.map(str::to_owned),
            noise_known: before.len() >= 2,
            noise: noise.into_iter().collect(),
            head_only_noise: head_only,
            diffs,
        });
    }
    (files, flags)
}

/// One version's records of `file` after the first pass against the second (plan, row 74), with
/// the probe's `key` and `ignore`, without the paths that differ between the measured repeats of
/// that version: they change from repeat to repeat on one state already.
pub(crate) fn passes(
    raw: &Raw,
    outputs: &Outputs,
    context: &Context<'_>,
    (file, side): (&str, Side),
    [first, second]: &[Vec<Value>; 2],
) -> Vec<Diff> {
    let rule = Rule::of(raw.probes.get(file));
    let measured = outputs.records.files.get(file);
    let repeats = measured.map(|this| lists(file, side, outputs, context, this));
    let noise = repeats.map_or_else(BTreeSet::new, |lists| changing(rule, &lists));
    let mut diffs = diff(rule, first, second);
    diffs.retain(|diff| !noise.contains(&diff.path));
    diffs
}

/// The probes that ran in this run and cannot show a change (plan, row 70); a probe that did not
/// run, such as an `on-failure` one of a passed run, is no dead probe.
pub(crate) fn dead(raw: &Raw, outputs: &Outputs) -> Vec<Flag> {
    let mut ran: BTreeMap<&str, &str> = BTreeMap::new();
    for (_, setup, repeats) in sides(raw) {
        let after_workload = repeats.iter().flat_map(|repeat| &repeat.probes);
        for probe in setup.probes.iter().chain(after_workload) {
            if let Some(file) = probe.out.file_name() {
                ran.entry(probe.name.as_str()).or_insert(file);
            }
        }
    }
    let mut flags = Vec::new();
    for (probe, file) in ran {
        let found = outputs.records.files.get(file).into_iter();
        let repeats =
            found.flat_map(|records| records.head.values().chain(records.before.values()));
        let records: Vec<&Value> = repeats.flatten().collect();
        let rule = Rule::of(raw.probes.get(file));
        let kept = |record: &&Value| rule.keeps(&mut String::new(), record);
        let reason = if records.is_empty() {
            DeadReason::NoRecords
        } else if !rule.ignore.is_empty() && !records.iter().any(kept) {
            DeadReason::AllIgnored
        } else {
            continue;
        };
        flags.push(Flag::DeadProbe {
            probe: probe.to_owned(),
            reason,
        });
    }
    flags
}

/// The lists of records of one file on one version, the pool's runs first, oldest first: one per
/// measured repeat, and one per setup origin for an `after-setup` file (plan, row 88).
fn lists<'a>(
    file: &str,
    side: Side,
    outputs: &'a Outputs,
    context: &'a Context<'_>,
    this: &'a FileRecords,
) -> Vec<&'a [Value]> {
    let mut lists = Vec::new();
    let mut origins = BTreeSet::new();
    for (report, records) in context.pool.iter().zip(&outputs.pool) {
        let Some(records) = records.as_ref().and_then(|records| records.files.get(file)) else {
            continue;
        };
        if let Some(from) = report.origin(side) {
            add(&mut lists, &mut origins, from, records.side(side));
        }
    }
    add(
        &mut lists,
        &mut origins,
        context.origin(side),
        this.side(side),
    );
    lists
}

fn add<'a>(
    lists: &mut Vec<&'a [Value]>,
    origins: &mut BTreeSet<&'a str>,
    from: &'a str,
    repeats: &'a BTreeMap<u32, Vec<Value>>,
) {
    for (repeat, records) in repeats {
        if *repeat == SETUP && !origins.insert(from) {
            continue;
        }
        lists.push(records);
    }
}

/// The paths that differ between neighbouring lists: what changes from repeat to repeat.
fn changing(rule: Rule<'_>, lists: &[&[Value]]) -> BTreeSet<String> {
    let pairs = lists
        .windows(2)
        .flat_map(|pair| diff(rule, pair[0], pair[1]));
    pairs.map(|diff| diff.path).collect()
}

/// The leaf differences of two lists of records of one file.
fn diff(rule: Rule<'_>, before: &[Value], head: &[Value]) -> Vec<Diff> {
    let mut diffs = Vec::new();
    for (at, before, head) in pairs(rule, before, head) {
        let (Some(before), Some(head)) = (before, head) else {
            if !rule.ignores("") {
                let side = |record: Option<&Value>| record.cloned().unwrap_or(Value::Null);
                diffs.push(Diff {
                    at,
                    path: String::new(),
                    before: side(before),
                    head: side(head),
                });
            }
            continue;
        };
        let mut leaves = Vec::new();
        differ(&mut String::new(), before, head, &mut leaves);
        for (path, before, head) in leaves {
            if !rule.ignores(&path) {
                diffs.push(Diff {
                    at: at.clone(),
                    path,
                    before,
                    head,
                });
            }
        }
    }
    diffs
}

type Pair<'v> = (At, Option<&'v Value>, Option<&'v Value>);

/// Records side by side: by position from 1, or with `key` by the value at that pointer, in the
/// order the values first appear; equal values pair in order. A record only on one side has `None`
/// on the other; a record without the pointer pairs under `null`.
fn pairs<'v>(rule: Rule<'_>, before: &'v [Value], head: &'v [Value]) -> Vec<Pair<'v>> {
    let Some(key) = rule.key else {
        let len = before.len().max(head.len());
        let at = |index: usize| At::Record {
            record: u32::try_from(index + 1).unwrap_or(u32::MAX),
        };
        let paired = (0..len).map(|index| (at(index), before.get(index), head.get(index)));
        return paired.collect();
    };
    let mut order: Vec<(String, Value)> = Vec::new();
    let mut groups: HashMap<String, (Vec<&Value>, Vec<&Value>)> = HashMap::new();
    for (is_head, records) in [(false, before), (true, head)] {
        for record in records {
            let value = record.pointer(key).cloned().unwrap_or(Value::Null);
            let text = value.to_string();
            let group = groups.entry(text.clone()).or_insert_with(|| {
                order.push((text, value));
                (Vec::new(), Vec::new())
            });
            let side = if is_head { &mut group.1 } else { &mut group.0 };
            side.push(record);
        }
    }
    let mut pairs = Vec::new();
    for (text, value) in order {
        let (before, head) = &groups[&text];
        for index in 0..before.len().max(head.len()) {
            let at = At::Key { key: value.clone() };
            pairs.push((at, before.get(index).copied(), head.get(index).copied()));
        }
    }
    pairs
}

/// Down both values together; an absent field equals `null`, so `null` against nothing is no diff.
fn differ(path: &mut String, before: &Value, head: &Value, out: &mut Vec<(String, Value, Value)>) {
    match (before, head) {
        (Value::Object(before), Value::Object(head)) => {
            let keys: BTreeSet<&String> = before.keys().chain(head.keys()).collect();
            for key in keys {
                let len = path.len();
                push_token(path, key);
                let (before, head) = (before.get(key), head.get(key));
                differ(path, before.unwrap_or(&NULL), head.unwrap_or(&NULL), out);
                path.truncate(len);
            }
        }
        (Value::Array(before), Value::Array(head)) => {
            for index in 0..before.len().max(head.len()) {
                let len = path.len();
                push_token(path, &index.to_string());
                let (before, head) = (before.get(index), head.get(index));
                differ(path, before.unwrap_or(&NULL), head.unwrap_or(&NULL), out);
                path.truncate(len);
            }
        }
        _ if before == head => {}
        _ => out.push((path.clone(), before.clone(), head.clone())),
    }
}

/// `/` and the token with `~` and `/` escaped (RFC 6901).
fn push_token(path: &mut String, token: &str) {
    path.push('/');
    path.push_str(&token.replace('~', "~0").replace('/', "~1"));
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn leaves_differ_along_json_pointers_and_absent_is_null() {
        let before = json!({"a": {"b/c": 1, "d": [1, 2], "e~": 1}, "f": null});
        let head = json!({"a": {"b/c": 2, "d": [1, 3, 4], "e~": 2}});
        let mut leaves = Vec::new();
        differ(&mut String::new(), &before, &head, &mut leaves);
        let paths: Vec<&str> = leaves.iter().map(|(path, _, _)| path.as_str()).collect();
        assert_eq!(paths, ["/a/b~1c", "/a/d/1", "/a/d/2", "/a/e~0"]);
        assert_eq!((&leaves[2].1, &leaves[2].2), (&Value::Null, &json!(4)));
    }

    #[test]
    fn keyed_records_pair_by_the_pointer_and_a_lone_record_shows() {
        let ignore = ["/updated_at".to_owned()];
        let rule = Rule {
            key: Some("/id"),
            ignore: &ignore,
        };
        let before = [
            json!({"id": 1, "s": "a", "updated_at": 1}),
            json!({"id": 2, "s": "a"}),
        ];
        let head = [
            json!({"id": 2, "s": "b"}),
            json!({"id": 1, "s": "a", "updated_at": 2}),
            json!({"id": 3}),
        ];
        let diffs = diff(rule, &before, &head);
        let ats: Vec<(&At, &str)> = diffs
            .iter()
            .map(|diff| (&diff.at, diff.path.as_str()))
            .collect();
        let expected = [
            (&At::Key { key: json!(2) }, "/s"),
            (&At::Key { key: json!(3) }, ""),
        ];
        assert_eq!(ats, expected);
        assert_eq!(diffs[1].before, Value::Null);
    }
}
