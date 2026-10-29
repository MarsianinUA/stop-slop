//! The quiet `--json` line (architecture plan, row 76): only the exceptions of a report, the count
//! of the rest, and the next steps as `Next`. No I/O: `dir` only builds the paths.

use std::collections::BTreeSet;

use camino::Utf8Path;

use crate::flags::{self, Flag};
use crate::grade;
use crate::next::{Next, Topic};
use crate::raw::{Failure, Phase, Raw};
use crate::schema::{
    Behaviour, Compared, Grade, Line, LineFile, LineMeasure, Measure, REPORT, Report, Verdict,
    diff_path,
};
use crate::{Before, Side};

/// At most this many records of a diff in the line; the rest is in `diff/<file>.json`.
const SAMPLE: usize = 3;

impl Report {
    pub fn line(&self, dir: &Utf8Path) -> Line {
        let shown: Vec<&Measure> = self
            .measures
            .iter()
            .filter(|measure| exception(measure))
            .collect();
        let quiet = self.measures.iter().filter(|measure| !exception(measure));
        let unmeasurable = quiet.filter(|measure| graded(measure) == Some(Grade::Unmeasurable));
        let next = self.next(&shown);
        let measures: Vec<LineMeasure> = shown.into_iter().map(line_measure).collect();
        let named = named(&self.flags);
        let files = self.behaviour.as_ref().map(|files| {
            let shown = files
                .iter()
                .filter(|file| !file.diffs.is_empty() || named.contains(file.file.as_str()));
            shown.map(|file| line_file(file, dir)).collect::<Vec<_>>()
        });
        let all = self.measures.len() + self.behaviour.as_ref().map_or(0, Vec::len);
        let shown = measures.len() + files.as_ref().map_or(0, Vec::len);
        Line {
            run: Some(self.run.clone()),
            report: Some(dir.join(REPORT)),
            seal: self.seal.clone(),
            attempts: self.attempts,
            comparisons: self.comparisons,
            measures,
            behaviour: files,
            flags: self.flags.clone(),
            needs: self.needs.clone(),
            failure: None,
            quiet: u32::try_from(all - shown).unwrap_or(u32::MAX),
            unmeasurable: u32::try_from(unmeasurable.count()).unwrap_or(u32::MAX),
            next,
        }
    }

    /// An `accept` template per `surprise` and `exceeded` that fits `maxTolerance`, then
    /// `guide expect` once for an `unmeasurable` entry: its bound is tighter than the noise.
    fn next(&self, shown: &[&Measure]) -> Vec<Next> {
        let max_tolerance = self.seal.as_ref().map(|seal| seal.max_tolerance);
        let accepts = shown.iter().filter_map(|measure| {
            let bound = grade::accept(measure, max_tolerance?)?;
            Some(Next::Accept {
                measure: measure.name.clone(),
                bound,
            })
        });
        let mut next: Vec<Next> = accepts.collect();
        if shown
            .iter()
            .any(|measure| graded(measure) == Some(Grade::Unmeasurable))
        {
            next.push(Next::Guide(Some(Topic::Expect)));
        }
        next
    }
}

impl Line {
    /// Flags from what the head collected before it failed; `raw` is `None` before the start.
    /// `low-repeats` counts the repeats that passed: the failed one gave no sample, and a failed
    /// setup ran none, so it has no `low-repeats`. `run` comes back `None`: `cli` sets it once the
    /// run has its folder.
    pub fn failed(failure: Failure, raw: Option<&Raw>, next: Vec<Next>) -> Line {
        let flags = raw.map(|raw| {
            let mut found = flags::facts(raw);
            found.retain(|flag| !on_failure(flag, &failure));
            let before = match Before::of(raw) {
                Before::Passed(repeats) => Some(repeats.len()),
                Before::Absent | Before::Failed(_) => None,
            };
            if !raw.head.is_empty() {
                let passed = raw.head.iter();
                let passed = passed.filter(|repeat| Some(repeat.repeat) != failure.repeat);
                found.extend(flags::low_repeats(passed.count(), before));
            }
            flags::order(&mut found);
            found
        });
        Line {
            flags: flags.unwrap_or_default(),
            failure: Some(failure),
            next,
            ..Line::default()
        }
    }
}

/// The probes of the phase of the head that failed ran `on-failure`: their outputs are in
/// `failure.probes`, so they give no `probe-failed` (T-13 stage 2 decision). A failed setup and
/// its probes have no repeat; in a failed repeat no `after-workload` probe runs. A step after `run`,
/// such as `report`, has no repeat either but ran no probe on its failure.
fn on_failure(flag: &Flag, failure: &Failure) -> bool {
    let in_run = matches!(failure.phase, Phase::Setup | Phase::Ready | Phase::Workload);
    in_run
        && matches!(flag, Flag::ProbeFailed { side, repeat, .. }
            if side == Side::Head.as_str() && *repeat == failure.repeat)
}

/// Optional Assumptions, «Исключение строки»: a grade other than `met`, but the `unmeasurable` of
/// a measure without an entry, which stays quiet; `change: unknown`; `postHoc` and `stale`, always.
/// Without `--before` a shift against the previous run; with a failed "before" nothing was graded.
fn exception(measure: &Measure) -> bool {
    let Some(expect) = &measure.expect else {
        let moved = matches!(
            measure.previous.verdict,
            Verdict::Improved | Verdict::Regressed | Verdict::Changed
        );
        return measure.before.is_none() && moved;
    };
    expect.post_hoc
        || expect.stale
        || match expect.grade {
            None => true,
            Some(Grade::Met) => false,
            Some(Grade::Unmeasurable) => expect.reason.is_some(),
            Some(Grade::Exceeded | Grade::Surprise) => true,
        }
}

fn graded(measure: &Measure) -> Option<Grade> {
    measure.expect.as_ref()?.grade
}

/// The files a flag names: they go into the line even without a diff.
fn named(flags: &[Flag]) -> BTreeSet<String> {
    let named = flags.iter().filter_map(|flag| match flag {
        Flag::NoResults { file: Some(file) }
        | Flag::BadResultLine { file, .. }
        | Flag::HeadOnlyNoise { file, .. } => Some(file.clone()),
        Flag::DeadProbe { probe, .. } | Flag::ProbeFailed { probe, .. } => {
            Some(format!("probe.{probe}.jsonl"))
        }
        _ => None,
    });
    named.collect()
}

fn line_measure(measure: &Measure) -> LineMeasure {
    LineMeasure {
        name: measure.name.clone(),
        unit: measure.unit,
        src: measure.src,
        median: measure.median,
        before: measure.before.as_ref().map(|before| Compared {
            median: before.median,
            verdict: before.verdict,
        }),
        previous: measure.previous.clone(),
        expect: measure.expect.clone(),
    }
}

fn line_file(file: &Behaviour, dir: &Utf8Path) -> LineFile {
    LineFile {
        file: file.file.clone(),
        src: file.src,
        noise_known: file.noise_known,
        diffs: u32::try_from(file.diffs.len()).unwrap_or(u32::MAX),
        sample: file.diffs.iter().take(SAMPLE).cloned().collect(),
        path: diff_path(dir, &file.file),
    }
}
