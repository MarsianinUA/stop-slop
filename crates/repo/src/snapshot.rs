use std::collections::BTreeSet;
use std::fs;
use std::io::{ErrorKind, Write as _};
use std::panic::resume_unwind;
use std::path::PathBuf;
use std::thread;

use camino::{Utf8Path, Utf8PathBuf};
use tempfile::NamedTempFile;

use crate::{Change, ChangedFile, LineRange, Oid, RepoError, diff, git};

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

    /// Bytes as git stores them, without CRLF filters. A missing file is `None`, and on `Rev`
    /// a folder too.
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
        let mut args = vec!["grep", "-l", "-z", "-w", "-F", "-I", "--no-color"];
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
}

impl WorkTree {
    /// Replaces an existing file through a temporary file and a rename, with the permissions
    /// of the old file. A missing file is `Io` with `NotFound`. A symlink becomes a plain file.
    pub fn write(&self, path: &Utf8Path, bytes: &[u8]) -> Result<(), RepoError> {
        let full = self.root.join(path);
        let io_error = RepoError::io(&full);
        let permissions = fs::metadata(&full).map_err(&io_error)?.permissions();
        let dir = full.parent().expect("a file in the repo has a folder");
        let mut temp = NamedTempFile::new_in(dir).map_err(&io_error)?;
        temp.write_all(bytes).map_err(&io_error)?;
        temp.persist(&full).map_err(|error| io_error(error.error))?;
        // After the rename: a read-only temp file on Windows could not be removed when the
        // rename fails, and would stay in the project.
        fs::set_permissions(&full, permissions).map_err(&io_error)
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
    /// `<oid> <type> <size>\n<bytes>\n`, or `<request> missing\n`.
    fn read(&self, paths: &[Utf8PathBuf]) -> Result<Vec<Option<Vec<u8>>>, RepoError> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let requests: String = paths
            .iter()
            .map(|path| format!("{}:{path}\n", self.oid))
            .collect();
        let out = git::run_with_input(&self.root, &["cat-file", "--batch"], requests.as_bytes())?;
        let unexpected = || git::unexpected("cat-file --batch");
        let mut rest = out.as_slice();
        let mut files = Vec::with_capacity(paths.len());
        for _ in paths {
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
            let mut fields = header.split(' ').skip(1);
            let kind = fields.next().ok_or_else(unexpected)?;
            let size: usize = fields
                .next()
                .and_then(|size| size.parse().ok())
                .ok_or_else(unexpected)?;
            let body = rest.get(..size).ok_or_else(unexpected)?;
            // A folder answers with the bytes of its tree: they are skipped all the same, or the
            // next answers would shift.
            files.push((kind == "blob").then(|| body.to_vec()));
            rest = rest.get(size + 1..).ok_or_else(unexpected)?;
        }
        Ok(files)
    }
}
