use std::collections::BTreeSet;
use std::fs;
use std::io::{self, ErrorKind, Write as _};
use std::panic::resume_unwind;
use std::path::PathBuf;
use std::thread;

use camino::{Utf8Path, Utf8PathBuf};
use tempfile::NamedTempFile;

use crate::{BranchDistance, Change, ChangedFile, LineRange, Oid, RepoError, diff, git};

/// Files on disk or files of a commit, with paths relative to the repo root.
#[derive(Debug)]
pub enum Snapshot {
    WorkTree(WorkTree),
    Rev(Rev),
}

/// The only writer of project files.
#[derive(Debug)]
pub struct WorkTree {
    pub(crate) root: PathBuf,
}

#[derive(Debug)]
pub struct Rev {
    pub(crate) root: PathBuf,
    pub(crate) oid: Oid,
}

impl Snapshot {
    pub fn head(&self) -> Result<Oid, RepoError> {
        match self {
            Snapshot::WorkTree(tree) => crate::commit(&tree.root, "HEAD"),
            Snapshot::Rev(rev) => Ok(rev.oid.clone()),
        }
    }

    /// The bytes on disk on `WorkTree`; on `Rev` the bytes git stores, without CRLF filters.
    /// Both follow a symlink. A missing file is `None`, and on `Rev` also a folder and a link
    /// that leads nowhere inside the revision.
    pub fn read(&self, paths: &[Utf8PathBuf]) -> Result<Vec<Option<Vec<u8>>>, RepoError> {
        match self {
            Snapshot::WorkTree(tree) => paths.iter().map(|path| tree.read(path)).collect(),
            Snapshot::Rev(rev) => rev.read(paths),
        }
    }

    /// Sorted. On `WorkTree`: tracked and untracked files without ignored ones; a file
    /// deleted from disk but still in the index is here too, and its `read` is `None`.
    pub fn files(&self) -> Result<Vec<Utf8PathBuf>, RepoError> {
        let out = match self {
            Snapshot::WorkTree(tree) => git::run(
                &tree.root,
                &[
                    "ls-files",
                    "-z",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                ],
            )?,
            Snapshot::Rev(rev) => git::run(
                &rev.root,
                &[
                    "ls-tree",
                    "-r",
                    "-z",
                    "--name-only",
                    "--full-tree",
                    &rev.oid.0,
                ],
            )?,
        };
        let mut files: Vec<Utf8PathBuf> = git::split(&out, 0).map(Into::into).collect();
        // git sorts by bytes, so `a.ts` comes before `a/b.ts`; `Utf8PathBuf` puts it after.
        // A conflicted file comes once per stage, and `--deduplicate` needs git 2.31.
        files.sort();
        files.dedup();
        Ok(files)
    }

    /// Files of `globs` changed since the merge base of `base` and this snapshot, with the
    /// changed lines of their new side, sorted by path. Files and lines come from one
    /// `diff -U0 --merge-base`; on `WorkTree` untracked files come from `ls-files --others`,
    /// run in parallel. Two merge bases (criss-cross) or none is `Git`.
    pub fn changes(&self, base: &Oid, globs: &[&str]) -> Result<Vec<ChangedFile>, RepoError> {
        let mut args = vec!["diff"];
        args.extend(diff::ARGS);
        args.extend(["--merge-base", &base.0]);
        let (root, others) = match self {
            Snapshot::WorkTree(tree) => {
                let mut others = vec!["ls-files", "-z", "--others", "--exclude-standard", "--"];
                others.extend(globs);
                (&tree.root, Some(others))
            }
            Snapshot::Rev(rev) => {
                args.push(&rev.oid.0);
                (&rev.root, None)
            }
        };
        args.push("--");
        args.extend(globs);
        let (patch, untracked) = match others {
            Some(others) => thread::scope(|scope| {
                let untracked = scope.spawn(|| git::run(root, &others));
                let patch = git::run(root, &args);
                let untracked = untracked
                    .join()
                    .unwrap_or_else(|panic| resume_unwind(panic));
                (patch, untracked)
            }),
            None => (git::run(root, &args), Ok(Vec::new())),
        };
        let (patch, untracked) = (patch?, untracked?);
        let mut files = diff::parse(&patch)?;
        files.extend(git::split(&untracked, 0).map(|path| ChangedFile {
            path: path.into(),
            change: Change::Untracked,
            lines: vec![LineRange {
                start: 1,
                end: u32::MAX,
            }],
        }));
        // A path comes twice when it is `Deleted` and is also on disk now: `git rm --cached`
        // leaves it untracked, a symlink turned into a file is also `Added`. The other entry
        // tells what the path holds now, so the `Deleted` one goes.
        let deleted = |file: &ChangedFile| file.change == Change::Deleted;
        files.sort_by(|a, b| a.path.cmp(&b.path).then(deleted(a).cmp(&deleted(b))));
        files.dedup_by(|later, earlier| later.path == earlier.path);
        Ok(files)
    }

    /// Files of `globs` whose content differs from `base` itself, not from the merge base:
    /// a file a squash merge already brought into `base` is not here. Untracked files are
    /// not here either.
    pub fn differs_from(
        &self,
        base: &Oid,
        globs: &[&str],
    ) -> Result<BTreeSet<Utf8PathBuf>, RepoError> {
        // `--no-renames` answers for every path on its own.
        let mut args = vec!["diff", "--name-only", "-z", "--no-renames", &base.0];
        let root = match self {
            Snapshot::WorkTree(tree) => &tree.root,
            Snapshot::Rev(rev) => {
                args.push(&rev.oid.0);
                &rev.root
            }
        };
        args.push("--");
        args.extend(globs);
        let out = git::run(root, &args)?;
        Ok(git::split(&out, 0).map(Into::into).collect())
    }

    /// Sorted files that contain `word` as a whole word, without binary files. On `WorkTree`
    /// untracked files count too.
    pub fn grep_word(&self, word: &str) -> Result<Vec<Utf8PathBuf>, RepoError> {
        // `submodule.recurse` in the user's config turns on `--recurse-submodules`, and
        // `--untracked` refuses to run with it.
        let mut args = vec![
            "grep",
            "-l",
            "-z",
            "-w",
            "-F",
            "-I",
            "--no-color",
            "--no-recurse-submodules",
        ];
        let (root, prefix) = match self {
            Snapshot::WorkTree(tree) => {
                args.extend(["--untracked", "-e", word]);
                (&tree.root, String::new())
            }
            Snapshot::Rev(rev) => {
                args.extend(["-e", word, &rev.oid.0]);
                // On a revision git prints `<oid>:<path>`.
                (&rev.root, format!("{}:", rev.oid))
            }
        };
        let Some(out) = git::run_found(root, &args)? else {
            return Ok(Vec::new());
        };
        let mut files: Vec<Utf8PathBuf> = git::split(&out, 0)
            .map(|line| line.strip_prefix(&prefix).unwrap_or(line).into())
            .collect();
        files.sort();
        Ok(files)
    }

    /// The branches of [`crate::Repo::branches`] that point to a commit, each with its
    /// distance to the head of this snapshot: `ahead` counts the commits of the branch that
    /// the head lacks, `behind` the commits of the head that the branch lacks, so `behind` is
    /// 0 when the branch contains the head. One `for-each-ref`; its `%(ahead-behind)` needs
    /// git 2.41.
    pub fn branch_distances(&self) -> Result<Vec<BranchDistance>, RepoError> {
        let (root, head) = match self {
            Snapshot::WorkTree(tree) => (&tree.root, "HEAD"),
            Snapshot::Rev(rev) => (&rev.root, rev.oid.0.as_str()),
        };
        let fields = format!("%(ahead-behind:{head})");
        let distances = crate::for_each_branch(root, &fields, |tip, counts| {
            // A ref to a tree or a blob gets no counts: git only warns about it and goes on.
            if counts.is_empty() {
                return Ok(None);
            }
            let (ahead, behind) = counts
                .split_once(' ')
                .and_then(|(ahead, behind)| Some((ahead.parse().ok()?, behind.parse().ok()?)))
                .ok_or_else(|| git::unexpected("for-each-ref"))?;
            Ok(Some(BranchDistance { tip, ahead, behind }))
        })
        .map_err(needs_new_git)?;
        Ok(distances.into_iter().flatten().collect())
    }
}

fn needs_new_git(error: RepoError) -> RepoError {
    match error {
        RepoError::Git { command, stderr }
            if stderr.contains("unknown field name: ahead-behind") =>
        {
            RepoError::Git {
                command,
                stderr: format!("{stderr}; stop-slop needs git 2.41 or newer"),
            }
        }
        error => error,
    }
}

impl WorkTree {
    /// Replaces an existing file through a temporary file and a rename, with the permissions
    /// of the old file. A missing file is `Io` with `NotFound`, a read-only one `Io` with
    /// `PermissionDenied`. A symlink becomes a plain file.
    pub fn write(&self, path: &Utf8Path, bytes: &[u8]) -> Result<(), RepoError> {
        let full = self.root.join(path);
        let io_error = RepoError::io(&full);
        let permissions = fs::metadata(&full).map_err(&io_error)?.permissions();
        if permissions.readonly() {
            // Windows cannot rename over a read-only file, while Linux and macOS would quietly
            // replace a file the user locked.
            let error = io::Error::new(ErrorKind::PermissionDenied, "the file is read-only");
            return Err(io_error(error));
        }
        let dir = full.parent().expect("a file in the repo has a folder");
        let mut temp = NamedTempFile::new_in(dir).map_err(&io_error)?;
        temp.write_all(bytes).map_err(&io_error)?;
        // Before the rename, so the new file never shows up with the mode of a temp file.
        temp.as_file()
            .set_permissions(permissions)
            .map_err(&io_error)?;
        temp.persist(&full).map_err(|error| io_error(error.error))?;
        Ok(())
    }

    fn read(&self, path: &Utf8Path) -> Result<Option<Vec<u8>>, RepoError> {
        let full = self.root.join(path);
        match fs::read(&full) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(source) => Err(RepoError::Io { path: full, source }),
        }
    }
}

impl Rev {
    /// One `cat-file --batch` for all paths. Its answers come in the order of the requests:
    /// `<oid> <type> <size>\n<bytes>\n`, or `<request> missing\n`. A symlink is followed inside
    /// the revision, as `fs::read` does on disk; a link that leads nowhere answers
    /// `<dangling|loop|notdir|symlink> <size>\n<bytes>\n`.
    fn read(&self, paths: &[Utf8PathBuf]) -> Result<Vec<Option<Vec<u8>>>, RepoError> {
        // A request is one line: a path with `\n` would split in two and shift every later
        // answer. Such a path is not asked and reads as missing.
        let asked = |path: &Utf8PathBuf| !path.as_str().contains('\n');
        let requests: String = paths
            .iter()
            .filter(|path| asked(path))
            .map(|path| format!("{}:{path}\n", self.oid))
            .collect();
        if requests.is_empty() {
            return Ok(vec![None; paths.len()]);
        }
        let out = git::run_with_input(
            &self.root,
            &["cat-file", "--batch", "--follow-symlinks"],
            requests.as_bytes(),
        )?;
        let unexpected = || git::unexpected("cat-file --batch");
        let mut rest = out.as_slice();
        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            if !asked(path) {
                files.push(None);
                continue;
            }
            let end = rest
                .iter()
                .position(|&byte| byte == b'\n')
                .ok_or_else(unexpected)?;
            let header = str::from_utf8(&rest[..end]).map_err(|_| unexpected())?;
            rest = &rest[end + 1..];
            if header.ends_with(" missing") {
                files.push(None);
                continue;
            }
            let (kind, size) = header.rsplit_once(' ').ok_or_else(unexpected)?;
            let size: usize = size.parse().map_err(|_| unexpected())?;
            let body = rest.get(..size).ok_or_else(unexpected)?;
            // A folder answers with the bytes of its tree: they are skipped all the same, or the
            // next answers would shift.
            files.push(kind.ends_with(" blob").then(|| body.to_vec()));
            rest = rest.get(size + 1..).ok_or_else(unexpected)?;
        }
        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_error(stderr: &str) -> RepoError {
        RepoError::Git {
            command: "git for-each-ref ...".to_owned(),
            stderr: stderr.to_owned(),
        }
    }

    #[test]
    fn needs_new_git_adds_a_hint_to_the_unknown_field_error() {
        let error = needs_new_git(git_error("fatal: unknown field name: ahead-behind:HEAD"));
        assert_eq!(
            error.to_string(),
            "`git for-each-ref ...` failed: fatal: unknown field name: ahead-behind:HEAD; \
             stop-slop needs git 2.41 or newer"
        );
    }

    #[test]
    fn needs_new_git_passes_other_errors_as_they_are() {
        let error = needs_new_git(git_error("fatal: failed to find 'HEAD'"));
        assert_eq!(
            error.to_string(),
            "`git for-each-ref ...` failed: fatal: failed to find 'HEAD'"
        );
        assert!(matches!(
            needs_new_git(RepoError::NotARepo),
            RepoError::NotARepo
        ));
    }
}
