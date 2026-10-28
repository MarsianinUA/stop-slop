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
    /// Also a rename (under the new path, with the lines of the edit only), a mode change
    /// and a binary file (no lines).
    Modified,
    Deleted,
    Untracked,
}

/// `lines` of `Untracked` is `1..=u32::MAX`; `Deleted` has none.
#[derive(Debug, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: Utf8PathBuf,
    pub change: Change,
    pub lines: Vec<LineRange>,
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
