use camino::{Utf8Path, Utf8PathBuf};
use repo::{Change, Oid, RepoError, Snapshot};

use crate::{Changed, ScopedFile, SkipReason, Skipped};

/// A `*` of a pathspec also matches `/`, so these reach every folder. They let `x.d.ts`
/// through: `checked` drops it.
const GLOBS: [&str; 4] = ["*.ts", "*.tsx", "*.mts", "*.cts"];
const DECLARATIONS: [&str; 3] = [".d.ts", ".d.mts", ".d.cts"];

/// The changed files since the merge base with `base`.
pub(crate) fn of_branch(
    snapshot: &Snapshot,
    base: &Oid,
) -> Result<(Vec<ScopedFile>, Vec<Skipped>), RepoError> {
    let (differs, changes) = crate::both(
        || snapshot.differs_from(base, &GLOBS),
        || snapshot.changes(base, &GLOBS),
    );
    let (changes, differs) = (changes?, differs?);
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for file in changes {
        if file.change == Change::Deleted || !checked(&file.path) {
            continue;
        }
        // `differs_from` does not list untracked files, and they are new anyway.
        if file.change != Change::Untracked && !differs.contains(&file.path) {
            skipped.push(Skipped {
                path: file.path,
                reason: SkipReason::AlreadyInBase,
            });
        } else {
            let new_path = matches!(
                file.change,
                Change::Added | Change::Renamed | Change::Untracked
            );
            files.push(ScopedFile {
                path: file.path,
                changed: Some(Changed {
                    lines: file.lines,
                    cuts: file.cuts,
                    new_path,
                }),
            });
        }
    }
    Ok((files, skipped))
}

pub(crate) fn listed(mut paths: Vec<Utf8PathBuf>) -> Vec<ScopedFile> {
    paths.retain(|path| checked(path));
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| ScopedFile {
            path,
            changed: None,
        })
        .collect()
}

pub(crate) fn checked(path: &Utf8Path) -> bool {
    let name = path.as_str();
    GLOBS
        .iter()
        .any(|glob| name.ends_with(glob.trim_start_matches('*')))
        && !DECLARATIONS.iter().any(|suffix| name.ends_with(suffix))
}
