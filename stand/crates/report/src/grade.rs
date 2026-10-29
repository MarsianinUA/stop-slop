//! The grade of a measure against its declared cost (architecture plan, `report`, «Оценка»; plan,
//! rows 66, 67). It reads the sign of the shift, not the measure's direction, so it grades neutral
//! measures too; the smallest shift that shows is the threshold of the verdict.

use crate::context::{Bound, Context, Declared};
use crate::flags::Flag;
use crate::schema::{ExpectJson, Grade, Measure, Need, Verdict};
use crate::stats::{self, Sample};

/// The tolerance of `none` and `down` (Config): a threshold above it cannot tell their bound from
/// noise.
const TOLERANCE: f64 = 0.05;

/// The head's median against "before".
#[derive(Debug, Clone, Copy)]
struct Shift {
    /// Relative to the median of "before"; infinite for a rise from 0.
    by: f64,
    shows: bool,
    threshold: f64,
}

/// `verdict` against "before", which passed with `base`.
pub(crate) fn expect(
    name: &str,
    verdict: Verdict,
    base: Sample,
    head: Sample,
    context: &Context<'_>,
) -> ExpectJson {
    let shift = Shift {
        by: shift(base.median, head.median),
        shows: shows(verdict),
        threshold: stats::threshold(base, head),
    };
    let accepted: Vec<&Declared> = context
        .accepted
        .iter()
        .filter(|entry| entry.measure == name)
        .collect();
    let here = context.head_tree.as_str();
    let effective = accepted
        .iter()
        .rev()
        .find(|entry| tree(entry) == Some(here));
    let elsewhere = accepted
        .iter()
        .any(|entry| tree(entry).is_some_and(|tree| tree != here));
    let stale = effective.is_none() && elsewhere;
    let declared = context.declared.iter().find(|entry| entry.measure == name);
    let applies = effective.copied().or(declared);
    let bound = applies.map_or(Bound::None, |entry| entry.bound);
    ExpectJson {
        change: bound.to_string(),
        reason: applies.map(|entry| entry.reason.clone()),
        grade: grade(bound, shift),
        post_hoc: effective.is_some(),
        stale,
        acceptances: u32::try_from(accepted.len()).unwrap_or(u32::MAX),
    }
}

/// The rules of «Оценка»: with a shift that shows, `met`, `exceeded` or `surprise`; within the noise,
/// `met` when the threshold fits the bound, else `unmeasurable`.
fn grade(bound: Bound, shift: Shift) -> Option<Grade> {
    let Shift {
        by,
        shows,
        threshold,
    } = shift;
    let fits = |tolerance: f64| {
        if threshold > tolerance {
            Grade::Unmeasurable
        } else {
            Grade::Met
        }
    };
    let bounded = |percent: u16| {
        if by.abs() <= fraction(percent) {
            Grade::Met
        } else {
            Grade::Exceeded
        }
    };
    let grade = match bound {
        Bound::Unknown => return None,
        Bound::None if shows => Grade::Surprise,
        Bound::None => fits(TOLERANCE),
        Bound::UpTo(percent) if !shows => fits(fraction(percent)),
        Bound::UpTo(_) if by < 0.0 => Grade::Surprise,
        Bound::UpTo(percent) => bounded(percent),
        Bound::DownTo(percent) if !shows => fits(fraction(percent)),
        Bound::DownTo(_) if by > 0.0 => Grade::Surprise,
        Bound::DownTo(percent) => bounded(percent),
        Bound::Down if shows && by < 0.0 => Grade::Met,
        Bound::Down if shows => Grade::Surprise,
        // A drop was declared and none showed, unless the noise could hide one.
        Bound::Down if threshold > TOLERANCE => Grade::Unmeasurable,
        Bound::Down => Grade::Surprise,
    };
    Some(grade)
}

/// The bound of the `accept` template of a `surprise` or an `exceeded` (architecture plan,
/// «Команды», `next`): the shift rounded up to a multiple of 5% with its sign, `none` within the
/// noise; none wider than `max_tolerance`, which `accept` would refuse.
pub(crate) fn accept(measure: &Measure, max_tolerance: u16) -> Option<Bound> {
    let expect = measure.expect.as_ref()?;
    if !matches!(expect.grade, Some(Grade::Surprise | Grade::Exceeded)) {
        return None;
    }
    let before = measure.before.as_ref()?;
    if !shows(before.verdict) {
        return Some(Bound::None);
    }
    let by = shift(before.median?, measure.median);
    // Less a hair: +55% is 11.000000000000002 times 5% as a float.
    let percent = ((by.abs() * 100.0 / 5.0) - 1e-9).ceil() * 5.0;
    if percent > f64::from(max_tolerance) {
        return None;
    }
    let percent = percent as u16;
    Some(if by > 0.0 {
        Bound::UpTo(percent)
    } else {
        Bound::DownTo(percent)
    })
}

pub(crate) fn needs(measures: &[Measure]) -> Vec<Need> {
    let unknown = measures.iter().filter_map(|measure| {
        let expect = measure
            .expect
            .as_ref()
            .filter(|expect| expect.grade.is_none())?;
        Some(Need {
            measure: measure.name.clone(),
            reason: expect.reason.clone().unwrap_or_default(),
        })
    });
    unknown.collect()
}

/// An entry of `expect.cost` that names no measure of the report grades nothing, which a typo
/// would hide.
pub(crate) fn unknown_expect(measures: &[Measure], context: &Context<'_>) -> Vec<Flag> {
    if context.before.is_none() {
        return Vec::new();
    }
    let missing = context.declared.iter().filter(|entry| {
        let found = measures.iter().any(|measure| measure.name == entry.measure);
        !found
    });
    let flags = missing.map(|entry| Flag::UnknownExpect {
        measure: entry.measure.clone(),
    });
    flags.collect()
}

/// `None` for an entry of `expect.cost`.
fn tree(entry: &Declared) -> Option<&str> {
    entry.post_hoc.as_ref().map(|post| post.head_tree.as_str())
}

fn shows(verdict: Verdict) -> bool {
    matches!(
        verdict,
        Verdict::Improved | Verdict::Regressed | Verdict::Changed
    )
}

/// From a median of 0 every rise is infinite, as every rise shows there.
fn shift(base: f64, head: f64) -> f64 {
    if base == 0.0 {
        if head == 0.0 {
            0.0
        } else {
            f64::INFINITY.copysign(head)
        }
    } else {
        (head - base) / base.abs()
    }
}

fn fraction(percent: u16) -> f64 {
    f64::from(percent) / 100.0
}
