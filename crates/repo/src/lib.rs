use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use camino::Utf8PathBuf;

mod diff;
mod git;
mod snapshot;

pub use snapshot::{Rev, Snapshot, WorkTree};

/// Its `Display` prints the full hex: 40 characters, or 64 in a SHA-256 repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Oid(String);

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `name` is `x` for `refs/heads/x` and `origin/x` for `refs/remotes/origin/x`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchTip {
    pub name: String,
    pub oid: Oid,
}

#[derive(Debug, PartialEq, Eq)]
pub struct BranchDistance {
    pub tip: BranchTip,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Added,
    /// Also a mode change and a binary file (no lines).
    Modified,
    /// Under the new path, with the lines of the edit only.
    Renamed,
    Deleted,
    Untracked,
}

/// `lines` of `Untracked` is `1..=u32::MAX`; `Deleted` has none.
#[derive(Debug, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: Utf8PathBuf,
    pub change: Change,
    pub lines: Vec<LineRange>,
    /// The hunks that took out a line of code, in order.
    pub cuts: Vec<Cut>,
}

/// A hunk on its new side: `count` lines from `start` stand where the old lines were, or with
/// `count = 0` the old lines went out after line `start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cut {
    pub start: u32,
    pub count: u32,
}

/// From 1, inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("not inside a git repository")]
    NotARepo,
    #[error("`{}` is outside the repository", .0.display())]
    OutsideRepo(PathBuf),
    #[error("`{0}` is not a commit")]
    UnknownRev(String),
    #[error("pull request {number} moved while it was fetched; run again")]
    PrMoved { number: u32 },
    #[error("`{command}` failed: {stderr}")]
    Git { command: String, stderr: String },
    #[error("{}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl RepoError {
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl Fn(io::Error) -> RepoError {
        let path = path.into();
        move |source| RepoError::Io {
            path: path.clone(),
            source,
        }
    }
}

#[derive(Debug)]
pub struct Repo {
    /// As git prints it: `C:/...` on Windows, never `\\?\C:\...` from `canonicalize`, which
    /// breaks `cmd` and the cwd of child processes.
    root: PathBuf,
    common_dir: PathBuf,
    cwd: PathBuf,
}

impl Repo {
    pub fn open(cwd: &Path) -> Result<Repo, RepoError> {
        let root = gitcmd::root(cwd)
            .map_err(git::repo_error)?
            .ok_or(RepoError::NotARepo)?;
        Ok(Repo {
            root: root.worktree,
            common_dir: root.common,
            cwd: cwd.to_owned(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Absolute, as git prints it: `C:/...` on Windows, never `\\?\C:\...`.
    pub fn git_common_dir(&self) -> &Path {
        &self.common_dir
    }

    /// Turns a user's path, absolute or relative to the `cwd` given to `open`, into a path
    /// of the repo with `/`. Symlinks are resolved on both sides, so the file must exist.
    /// A `\` is a separator only on Windows: on Linux and macOS it stays part of the name.
    pub fn relativize(&self, path: &Path) -> Result<Utf8PathBuf, RepoError> {
        let io_error = RepoError::io(path);
        let full = fs::canonicalize(self.cwd.join(path)).map_err(&io_error)?;
        let root = fs::canonicalize(&self.root).map_err(RepoError::io(&self.root))?;
        let rest = full
            .strip_prefix(&root)
            .map_err(|_| RepoError::OutsideRepo(path.to_owned()))?;
        let names: Vec<&str> = rest
            .components()
            .map(|component| component.as_os_str().to_str())
            .collect::<Option<_>>()
            .ok_or_else(|| {
                io_error(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the path is not UTF-8",
                ))
            })?;
        Ok(Utf8PathBuf::from(names.join("/")))
    }

    pub fn worktree(&self) -> Snapshot {
        Snapshot::WorkTree(WorkTree {
            root: self.root.clone(),
        })
    }

    pub fn at_rev(&self, rev: &str) -> Result<Snapshot, RepoError> {
        let oid = self
            .resolve(rev)?
            .ok_or_else(|| RepoError::UnknownRev(rev.to_owned()))?;
        Ok(Snapshot::Rev(Rev {
            root: self.root.clone(),
            oid,
        }))
    }

    /// `None` for a name that is not a commit.
    pub fn resolve(&self, rev: &str) -> Result<Option<Oid>, RepoError> {
        let spec = commit_spec(rev);
        let out = git::run_found(&self.root, &["rev-parse", "--verify", "--quiet", &spec])?;
        Ok(out.map(oid))
    }

    /// `None` on a detached HEAD.
    pub fn current_branch(&self) -> Result<Option<String>, RepoError> {
        let out = git::run_found(&self.root, &["symbolic-ref", "--quiet", "HEAD"])?;
        Ok(out.map(|out| {
            let name = text(out);
            match name.strip_prefix("refs/heads/") {
                Some(short) => short.to_owned(),
                None => name,
            }
        }))
    }

    /// Local branches and the ones of `origin`, in the order of ref names; `origin/HEAD`,
    /// other remotes and tags are left out.
    pub fn branches(&self) -> Result<Vec<BranchTip>, RepoError> {
        for_each_branch(&self.root, "", |tip, _| Ok(tip))
    }

    /// The branch `origin/HEAD` points to; `None` when there is no `origin/HEAD`.
    pub fn default_branch(&self) -> Result<Option<BranchTip>, RepoError> {
        let out = git::run(
            &self.root,
            &[
                "for-each-ref",
                "--format=%(symref) %(objectname)",
                "refs/remotes/origin/HEAD",
            ],
        )?;
        let tip = git::split(&out, b'\n').next().and_then(|line| {
            let (target, oid) = line.split_once(' ')?;
            Some(BranchTip {
                name: target.strip_prefix("refs/remotes/")?.to_owned(),
                oid: Oid(oid.to_owned()),
            })
        });
        Ok(tip)
    }

    /// `None` when the histories have no common commit.
    pub fn merge_base(&self, a: &Oid, b: &Oid) -> Result<Option<Oid>, RepoError> {
        let out = git::run_found(&self.root, &["merge-base", &a.0, &b.0])?;
        Ok(out.map(oid))
    }

    /// Whether `tree` is the full oid of a tree in this repository.
    pub fn has_tree(&self, tree: &str) -> Result<bool, RepoError> {
        // A name or an option is not asked: only hex can be a full oid.
        if tree.is_empty() || !tree.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(false);
        }
        // `^{tree}` reads the object, so the oid of no object fails; a commit or an abbreviated
        // oid resolves to another text than `tree`.
        let spec = format!("{tree}^{{tree}}");
        let out = git::run_found(&self.root, &["rev-parse", "--verify", "--quiet", &spec])?;
        Ok(out.is_some_and(|out| text(out) == tree))
    }

    /// Of `paths`, the files on disk that are not their blob in `tree`; missing on one side
    /// counts. Blob against blob, not `git diff <tree>`, which takes an untracked file that went
    /// into `tree` for deleted. `hash-object` cleans a file as `git add` does, so a line ending
    /// that `core.autocrlf` turns back is no change.
    pub fn changed_since(
        &self,
        tree: &str,
        paths: &[Utf8PathBuf],
    ) -> Result<BTreeSet<Utf8PathBuf>, RepoError> {
        // Both commands read a path per line, and `hash-object` unquotes one that starts with
        // `"`: such a path cannot be asked, and counts as changed.
        let (asked, odd): (Vec<&Utf8PathBuf>, Vec<&Utf8PathBuf>) = paths
            .iter()
            .partition(|path| !path.as_str().contains('\n') && !path.as_str().starts_with('"'));
        let stored = blobs(&self.root, tree, &asked)?;
        let exists: Vec<bool> = asked
            .iter()
            .map(|path| self.root.join(path).is_file())
            .collect();
        let on_disk: Vec<&Utf8PathBuf> = asked
            .iter()
            .zip(&exists)
            .filter_map(|(path, &exists)| exists.then_some(*path))
            .collect();
        let mut hashes = hash_files(&self.root, &on_disk)?.into_iter();
        let mut changed: BTreeSet<Utf8PathBuf> = odd.into_iter().cloned().collect();
        for ((path, stored), exists) in asked.into_iter().zip(stored).zip(exists) {
            let now = if exists { hashes.next() } else { None };
            if now != stored {
                changed.insert(path.clone());
            }
        }
        Ok(changed)
    }

    /// A fetched head other than `head` is `PrMoved`: a push came between `gh` and the fetch.
    pub fn fetch_pr(&self, number: u32, base: &str, head: &str) -> Result<(), RepoError> {
        let pr = format!("refs/stop-slop/pr-{number}");
        // Only the two refs the check reads: no tags, no fetch in submodules, which can fail
        // the whole run, and no `maintenance --auto`, whose gc cannot detach on Windows.
        git::run(
            &self.root,
            &[
                "fetch",
                "--no-tags",
                "--no-recurse-submodules",
                "--no-auto-maintenance",
                "origin",
                &format!("+refs/pull/{number}/head:{pr}"),
                &format!("+refs/heads/{base}:refs/remotes/origin/{base}"),
            ],
        )?;
        if commit(&self.root, &pr)?.0 != head {
            return Err(RepoError::PrMoved { number });
        }
        Ok(())
    }
}

/// `fields` go into the format after the name, and `parse` gets what git printed for them.
fn for_each_branch<T>(
    root: &Path,
    fields: &str,
    mut parse: impl FnMut(BranchTip, &str) -> Result<T, RepoError>,
) -> Result<Vec<T>, RepoError> {
    // `lstrip=2` turns `refs/heads/x` into `x` and `refs/remotes/origin/x` into `origin/x`.
    let format = format!("--format=%(symref) %(objectname) %(refname:lstrip=2) {fields}");
    let out = git::run(
        root,
        &["for-each-ref", &format, "refs/heads", "refs/remotes/origin"],
    )?;
    let mut branches = Vec::new();
    for line in git::split(&out, b'\n') {
        // `symref` is empty unless the ref is symbolic; a ref name has no spaces.
        let mut parts = line.splitn(4, ' ');
        let (Some(""), Some(oid), Some(name)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let tip = BranchTip {
            name: name.to_owned(),
            oid: Oid(oid.to_owned()),
        };
        branches.push(parse(tip, parts.next().unwrap_or(""))?);
    }
    Ok(branches)
}

/// The oid of the blob at each of `paths` in `tree`; `None` where `tree` has none there, also
/// for a folder.
fn blobs(
    root: &Path,
    tree: &str,
    paths: &[&Utf8PathBuf],
) -> Result<Vec<Option<String>>, RepoError> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let requests: String = paths
        .iter()
        .map(|path| format!("{tree}:{path}\n"))
        .collect();
    let out = git::run_with_input(root, &["cat-file", "--batch-check"], requests.as_bytes())?;
    // An answer per request, in order: `<oid> <type> <size>` or `<request> missing`.
    let answers: Vec<&[u8]> = out.split(|&byte| byte == b'\n').take(paths.len()).collect();
    if answers.len() != paths.len() {
        return Err(git::unexpected("cat-file --batch-check"));
    }
    let blob = |answer: &[u8]| {
        let answer = str::from_utf8(answer).ok()?;
        if answer.ends_with(" missing") {
            return None;
        }
        let mut fields = answer.split(' ');
        let (oid, kind) = (fields.next()?, fields.next()?);
        (kind == "blob").then(|| oid.to_owned())
    };
    Ok(answers.into_iter().map(blob).collect())
}

/// The blob oids `git add` would store for `paths`, in order.
fn hash_files(root: &Path, paths: &[&Utf8PathBuf]) -> Result<Vec<String>, RepoError> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let input: String = paths.iter().map(|path| format!("{path}\n")).collect();
    let out = git::run_with_input(root, &["hash-object", "--stdin-paths"], input.as_bytes())?;
    let oids: Vec<String> = git::split(&out, b'\n').map(str::to_owned).collect();
    if oids.len() != paths.len() {
        return Err(git::unexpected("hash-object --stdin-paths"));
    }
    Ok(oids)
}

fn commit(root: &Path, rev: &str) -> Result<Oid, RepoError> {
    let spec = commit_spec(rev);
    git::run(root, &["rev-parse", "--verify", &spec]).map(oid)
}

/// `^{commit}` peels an annotated tag to its commit and makes `-x` a bad revision instead of
/// an option of `rev-parse`.
fn commit_spec(rev: &str) -> String {
    format!("{rev}^{{commit}}")
}

fn oid(stdout: Vec<u8>) -> Oid {
    Oid(text(stdout))
}

fn text(stdout: Vec<u8>) -> String {
    String::from_utf8_lossy(&stdout).trim_end().to_owned()
}
