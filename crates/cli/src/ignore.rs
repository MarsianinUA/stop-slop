use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use state::{IgnoreEntry, State};

use crate::args::CheckArgs;
use crate::check;
use crate::error::{CliError, Exit};
use crate::output;

pub(crate) fn run(
    fingerprint: String,
    reason: String,
    base: Option<String>,
) -> Result<Exit, CliError> {
    let repo = crate::cwd_repo()?;
    let state = State::open(repo.git_common_dir());
    // Without the exceptions: an entry already made would hide its finding, and `ignore` of
    // it again, with a new reason, would fail. `add_ignore` replaces the entry.
    let engine = crate::new_engine(&rules::RULESET, &state, BTreeSet::new())?;
    let args = CheckArgs {
        base,
        ..CheckArgs::default()
    };
    let checked = check::scan(&repo, &engine, args)?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let added = utc_date(now);
    let mut entries: Vec<IgnoreEntry> = checked
        .findings
        .into_iter()
        .filter(|finding| finding.fingerprint.to_string() == fingerprint)
        .map(|finding| IgnoreEntry {
            fingerprint: fingerprint.clone(),
            rule: finding.rule.to_owned(),
            file: finding.file,
            reason: reason.clone(),
            added: added.clone(),
        })
        .collect();
    // Two equal places in one function share a fingerprint; the findings are sorted by file.
    entries.dedup_by(|a, b| a.file == b.file);
    if entries.is_empty() {
        return Err(CliError::UnknownFingerprint(fingerprint));
    }

    let text = output::ignore_text(&entries);
    state.add_ignore(entries)?;
    output::print(&text)?;
    Ok(Exit::Clean)
}

/// `YYYY-MM-DD` in UTC: `civil_from_days` of H. Hinnant, unsigned since no day is before 1970.
fn utc_date(unix_seconds: u64) -> String {
    let days = unix_seconds / 86_400;
    // Days from 0000-03-01: a year then ends with February and its leap day.
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = era * 400 + year_of_era + u64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::utc_date;

    #[test]
    fn utc_date_counts_leap_years() {
        for (seconds, date) in [
            (0, "1970-01-01"),
            (951_782_400, "2000-02-29"),
            (951_868_800, "2000-03-01"),
            (4_107_456_000, "2100-02-28"),
            (4_107_542_400, "2100-03-01"),
            (1_792_022_399, "2026-10-14"),
            (1_792_022_400, "2026-10-15"),
        ] {
            assert_eq!(utc_date(seconds), date, "{seconds}");
        }
    }
}
