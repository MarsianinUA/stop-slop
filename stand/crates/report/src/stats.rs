//! The statistics of the report, pure code (architecture plan, rows 11 and 68; Optional
//! Assumptions, «Вердикт»). An attempt is the samples of one run of the pool on one version.

use crate::schema::{Direction, Verdict};

/// The smallest shift a verdict ever tells from noise: 5% of the base median.
pub(crate) const MIN_SHIFT: f64 = 0.05;

/// The median of the pooled samples and the spread of their attempts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Sample {
    pub(crate) median: f64,
    pub(crate) spread: f64,
}

impl Sample {
    /// `None` without a single sample.
    pub(crate) fn of(attempts: &[&[f64]]) -> Option<Sample> {
        Some(Sample {
            median: median(&pooled(attempts))?,
            spread: spread(attempts),
        })
    }
}

pub(crate) fn pooled(attempts: &[&[f64]]) -> Vec<f64> {
    attempts.concat()
}

pub(crate) fn median(samples: &[f64]) -> Option<f64> {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    match sorted.len() {
        0 => None,
        len if len % 2 == 1 => Some(sorted[middle]),
        _ => Some((sorted[middle - 1] + sorted[middle]) / 2.0),
    }
}

/// (max - min) / |median| of one attempt. With a median of 0 the larger end stands in, so a rise
/// from zero is a range of 1, not an infinite one.
pub(crate) fn range(samples: &[f64]) -> f64 {
    let low = samples.iter().copied().reduce(f64::min);
    let high = samples.iter().copied().reduce(f64::max);
    let (Some(low), Some(high)) = (low, high) else {
        return 0.0;
    };
    if high == low {
        return 0.0;
    }
    let middle = median(samples).unwrap_or(0.0).abs();
    let base = if middle > 0.0 {
        middle
    } else {
        low.abs().max(high.abs())
    };
    (high - low) / base
}

/// The median of the ranges of the attempts (plan, row 68): pooling more attempts does not widen
/// it. An attempt of one sample has no range; when every attempt has one sample (`repeat: 1`), the
/// range of those samples stands in, or the spread would claim no noise at all.
pub(crate) fn spread(attempts: &[&[f64]]) -> f64 {
    let ranges: Vec<f64> = attempts
        .iter()
        .filter(|attempt| attempt.len() >= 2)
        .map(|attempt| range(attempt))
        .collect();
    match median(&ranges) {
        Some(spread) => spread,
        None => range(&pooled(attempts)),
    }
}

/// The smallest shift of `now` against `base` that is not noise.
pub(crate) fn threshold(base: Sample, now: Sample) -> f64 {
    MIN_SHIFT.max(base.spread).max(now.spread)
}

/// A shift of the medians above `threshold` shows (Optional Assumptions, «Вердикт»): a `lower`
/// measure improves or regresses, a `neutral` one only changes.
pub(crate) fn verdict(direction: Direction, base: Sample, now: Sample) -> Verdict {
    let rise = if base.median == 0.0 {
        if now.median == 0.0 {
            return Verdict::WithinNoise;
        }
        // From zero every rise shows: `error-lines` and `http-5xx-share` regress whatever the
        // noise.
        now.median > 0.0
    } else {
        let shift = (now.median - base.median) / base.median.abs();
        if shift.abs() <= threshold(base, now) {
            return Verdict::WithinNoise;
        }
        shift > 0.0
    };
    match (direction, rise) {
        (Direction::Neutral, _) => Verdict::Changed,
        (Direction::Lower, true) => Verdict::Regressed,
        (Direction::Lower, false) => Verdict::Improved,
    }
}

/// Shares and spreads keep 4 places, the times of hot functions 1.
pub(crate) fn round(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(median: f64, spread: f64) -> Sample {
        Sample { median, spread }
    }

    #[test]
    fn the_median_of_an_even_count_is_the_mean_of_the_middle_two() {
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), Some(2.5));
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn the_spread_is_the_median_of_the_ranges_of_the_attempts() {
        let one: &[f64] = &[90.0, 100.0, 110.0];
        assert_eq!(range(one), 0.2);
        assert_eq!(spread(&[one]), 0.2);
        // Three attempts of the same range, far apart from each other, keep the spread of one.
        let two: &[f64] = &[180.0, 200.0, 220.0];
        let three: &[f64] = &[45.0, 50.0, 55.0];
        assert_eq!(spread(&[one, two, three]), 0.2);
        let calm: &[f64] = &[100.0, 100.0, 100.0];
        assert_eq!(spread(&[one, calm, calm]), 0.0);
    }

    #[test]
    fn single_sample_attempts_take_the_range_of_their_samples() {
        let attempts: [&[f64]; 3] = [&[90.0], &[100.0], &[110.0]];
        assert_eq!(spread(&attempts), 0.2);
        assert_eq!(spread(&[&[100.0]]), 0.0);
    }

    #[test]
    fn a_range_from_zero_is_one() {
        assert_eq!(range(&[0.0, 0.0, 2.0]), 1.0);
        assert_eq!(range(&[0.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn the_threshold_is_five_percent_or_the_wider_spread() {
        assert_eq!(
            threshold(sample(100.0, 0.01), sample(100.0, 0.02)),
            MIN_SHIFT
        );
        assert_eq!(threshold(sample(100.0, 0.08), sample(100.0, 0.12)), 0.12);
        // 6% above a 5% threshold shows; under spreads of 8% it does not.
        let lower = Direction::Lower;
        assert_eq!(
            verdict(lower, sample(100.0, 0.0), sample(106.0, 0.0)),
            Verdict::Regressed
        );
        assert_eq!(
            verdict(lower, sample(100.0, 0.08), sample(106.0, 0.0)),
            Verdict::WithinNoise
        );
        assert_eq!(
            verdict(lower, sample(100.0, 0.0), sample(94.0, 0.0)),
            Verdict::Improved
        );
        assert_eq!(
            verdict(lower, sample(100.0, 0.0), sample(105.0, 0.0)),
            Verdict::WithinNoise
        );
    }

    #[test]
    fn pooled_attempts_join_their_samples() {
        let attempts: [&[f64]; 2] = [&[1.0, 2.0], &[3.0]];
        assert_eq!(pooled(&attempts), [1.0, 2.0, 3.0]);
        assert_eq!(
            Sample::of(&attempts),
            Some(Sample {
                median: 2.0,
                spread: 1.0 / 1.5
            })
        );
    }
}
