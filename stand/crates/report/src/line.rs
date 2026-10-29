//! The quiet `--json` line (architecture plan, row 76): only the exceptions of a report, the count
//! of the rest, and the next steps as `Next`. No I/O: `dir` only builds the paths.

use std::collections::BTreeSet;

use camino::Utf8Path;

use crate::Side;
use crate::flags::{self, Flag};
use crate::next::Next;
use crate::raw::{BeforeOutcome, Failure, Raw};
use crate::schema::{
    Behaviour, Compared, Line, LineFile, LineMeasure, Measure, REPORT, Report, Verdict, diff_path,
};

/// At most this many records of a diff in the line; the rest is in `diff/<file>.json`.
const SAMPLE: usize = 3;

impl Report {
    pub fn line(&self, dir: &Utf8Path) -> Line {
        let measures = self.measures.iter().filter(|measure| moved(measure));
        let measures: Vec<LineMeasure> = measures.map(line_measure).collect();
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
            measures,
            behaviour: files,
            flags: self.flags.clone(),
            failure: None,
            quiet: u32::try_from(all - shown).unwrap_or(u32::MAX),
            unmeasurable: 0,
            next: Vec::new(),
        }
    }
}

impl Line {
    /// Flags from what the head collected before it failed; `raw` is `None` before the start. A
    /// failed setup has no repeat, so no `low-repeats`. `run` comes back `None`: `cli` sets it once
    /// the run has its folder.
    pub fn failed(failure: Failure, raw: Option<&Raw>, next: Vec<Next>) -> Line {
        let flags = raw.map(|raw| {
            let mut found = flags::facts(raw);
            found.retain(|flag| !on_failure(flag, &failure));
            let before = match &raw.before {
                Some(BeforeOutcome::Passed(repeats)) => Some(repeats.len()),
                Some(BeforeOutcome::Failed(_)) | None => None,
            };
            if !raw.head.is_empty() {
                found.extend(flags::low_repeats(raw.head.len(), before));
            }
            flags::order(&mut found);
            found
        });
        Line {
            run: None,
            report: None,
            measures: Vec::new(),
            behaviour: None,
            flags: flags.unwrap_or_default(),
            failure: Some(failure),
            quiet: 0,
            unmeasurable: 0,
            next,
        }
    }
}

/// The probes of the phase of the head that failed ran `on-failure`: their outputs are in
/// `failure.probes`, so they give no `probe-failed` (T-13 stage 2 decision). A failed setup and
/// its probes have no repeat; in a failed repeat no `after-workload` probe runs.
fn on_failure(flag: &Flag, failure: &Failure) -> bool {
    matches!(flag, Flag::ProbeFailed { side, repeat, .. }
        if side == Side::Head.as_str() && *repeat == failure.repeat)
}

/// A shift that shows: against "before" with `--before`, else against the previous run. The grades
/// of T-13b replace this rule.
fn moved(measure: &Measure) -> bool {
    let before = measure.before.as_ref();
    let verdict = before.map_or(measure.previous.verdict, |before| before.verdict);
    matches!(
        verdict,
        Verdict::Improved | Verdict::Regressed | Verdict::Changed
    )
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
