//! The work tree as a git tree, the stand's snapshot of it, the tree "before" and trees as tar.
//! Nothing here touches the index or the files of the work tree.

use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use camino::{Utf8Path, Utf8PathBuf};
use gitcmd::{Git, GitError};
use sha2::{Digest, Sha256};

use crate::index::TempIndex;

mod archive;
mod index;

/// Refs live in the common dir, which every worktree of a repo shares: one ref per worktree key.
const BEFORE_REF: &str = "refs/stop-slop/stand/before/";

/// The author and committer of a snapshot commit: it is the tool's commit, and a host without
/// `user.name` still pins.
const IDENTITY: (&str, &str) = ("stop-slop-stand", "stop-slop-stand@localhost");

pub fn open(cwd: &Utf8Path) -> Result<Worktree, SourceError> {
    let found = gitcmd::root(cwd.as_std_path())?.ok_or(SourceError::NotARepo)?;
    let root = utf8(found.worktree)?;
    let canonical = root
        .canonicalize_utf8()
        .map_err(SourceError::io(root.as_std_path()))?;
    let hash = hex(&Sha256::digest(canonical.as_str()));
    Ok(Worktree {
        root,
        common: utf8(found.common)?,
        key: WorktreeKey(hash[..12].to_owned()),
    })
}

/// The tree of the files on disk: tracked and untracked ones, without ignored ones.
pub fn snapshot(worktree: &Worktree) -> Result<Tree, SourceError> {
    let index = TempIndex::copy_of(&worktree.git())?;
    let git = worktree.git().env("GIT_INDEX_FILE", index.path());
    git.run(&["add", "--all"])?;
    Ok(Tree(text(git.run(&["write-tree"])?)))
}

/// A commit without parents over `tree`, dated `now`, and `refs/stop-slop/stand/before/<key>`
/// on it.
pub fn pin(worktree: &Worktree, tree: &Tree, now: SystemTime) -> Result<Pinned, SourceError> {
    let seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let date = format!("@{seconds} +0000");
    let (name, email) = IDENTITY;
    let git = worktree
        .git()
        .env("GIT_AUTHOR_NAME", name)
        .env("GIT_AUTHOR_EMAIL", email)
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_NAME", name)
        .env("GIT_COMMITTER_EMAIL", email)
        .env("GIT_COMMITTER_DATE", &date);
    let reference = worktree.before_ref();
    let replaced = find_commit(&git, &reference)?;
    // `commit-tree` signs when `commit.gpgSign` is set, and a snapshot must not ask for a key.
    let out = git.run(&[
        "commit-tree",
        "--no-gpg-sign",
        "-m",
        "stop-slop-stand snapshot",
        &tree.0,
    ])?;
    let commit = Commit(text(out));
    // The old value makes a pin in between fail instead of being replaced unseen; empty means
    // the ref must not exist.
    let old = replaced.as_ref().map_or("", |commit| commit.0.as_str());
    git.run(&["update-ref", &reference, &commit.0, old])?;
    Ok(Pinned { commit, replaced })
}

pub fn before(worktree: &Worktree, before: &Before) -> Result<Resolved, SourceError> {
    let git = worktree.git();
    let treeish = match before {
        Before::Session => {
            find_commit(&git, &worktree.before_ref())?.ok_or(SourceError::NoSnapshot)?
        }
        Before::Ref(name) => {
            let tip =
                find_commit(&git, name)?.ok_or_else(|| SourceError::UnknownRef(name.clone()))?;
            let no_base = || SourceError::NoMergeBase(name.clone());
            // An unborn HEAD shares no commit either, and `merge-base` fails on it with 128.
            let head = find_commit(&git, "HEAD")?.ok_or_else(no_base)?;
            let base = git
                .run_found(&["merge-base", &head.0, &tip.0])?
                .ok_or_else(no_base)?;
            Commit(text(base))
        }
    };
    let (tree, taken_at) = read_commit(&git, &treeish)?;
    Ok(Resolved {
        intent: before.clone(),
        treeish,
        tree,
        taken_at,
    })
}

/// The tar of `tree` from its git objects: modes, the executable bit, symlinks as symlinks; no
/// `export-ignore` or `export-subst`, unlike `git archive`.
pub fn archive(worktree: &Worktree, tree: &Tree, out: &mut dyn Write) -> Result<(), SourceError> {
    archive::write(&worktree.git(), tree, out)
}

/// Paths from the root that differ between the trees, added and removed ones included:
/// `git diff-tree -r --name-only -z`.
pub fn changed(
    worktree: &Worktree,
    before: &Tree,
    head: &Tree,
) -> Result<Vec<Utf8PathBuf>, SourceError> {
    let out = worktree
        .git()
        .run(&["diff-tree", "-r", "--name-only", "-z", &before.0, &head.0])?;
    Ok(out
        .split(|&byte| byte == 0)
        .filter(|path| !path.is_empty())
        // Only compared with the paths of profiles, which are UTF-8.
        .map(|path| String::from_utf8_lossy(path).into_owned().into())
        .collect())
}

/// A missing ref is not an error.
pub fn forget(worktree: &Worktree) -> Result<(), SourceError> {
    worktree
        .git()
        .run(&["update-ref", "-d", &worktree.before_ref()])?;
    Ok(())
}

#[derive(Debug)]
pub struct Worktree {
    pub root: Utf8PathBuf,
    pub common: Utf8PathBuf,
    pub key: WorktreeKey,
}

impl Worktree {
    fn git(&self) -> Git {
        Git::at(self.root.as_std_path())
    }

    fn before_ref(&self) -> String {
        format!("{BEFORE_REF}{}", self.key)
    }
}

/// The first 12 hex of sha256 of the canonical root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeKey(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Before {
    Session,
    Ref(String),
}

/// Full hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit(String);

#[derive(Debug)]
pub struct Pinned {
    pub commit: Commit,
    pub replaced: Option<Commit>,
}

#[derive(Debug)]
pub struct Resolved {
    pub intent: Before,
    /// The snapshot commit, or `git merge-base HEAD <ref>`.
    pub treeish: Commit,
    pub tree: Tree,
    /// The snapshot's time for `Session`, the merge-base's commit date for `Ref`; whole seconds.
    pub taken_at: SystemTime,
}

impl fmt::Display for WorktreeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for Tree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for Commit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("not inside a git work tree")]
    NotARepo,
    #[error("no snapshot of this work tree: run `stop-slop-stand snapshot` first")]
    NoSnapshot,
    #[error("`{0}` is not a commit")]
    UnknownRef(String),
    #[error("HEAD and `{0}` have no common commit")]
    NoMergeBase(String),
    #[error(transparent)]
    Git(#[from] GitError),
    /// `out` of `archive` failed; only its owner knows its path.
    #[error("cannot write the tar: {0}")]
    Write(#[source] io::Error),
    #[error("{path}: {source}")]
    Io {
        path: Utf8PathBuf,
        #[source]
        source: io::Error,
    },
}

impl SourceError {
    fn io(path: &Path) -> impl FnOnce(io::Error) -> SourceError {
        move |source| SourceError::Io {
            path: path.to_string_lossy().into_owned().into(),
            source,
        }
    }
}

/// `^{commit}` peels an annotated tag to its commit and makes `-x` a bad revision instead of an
/// option of `rev-parse`.
fn find_commit(git: &Git, name: &str) -> Result<Option<Commit>, SourceError> {
    let spec = format!("{name}^{{commit}}");
    let out = git.run_found(&["rev-parse", "--verify", "--quiet", &spec])?;
    Ok(out.map(|out| Commit(text(out))))
}

/// The tree and the committer date from the header of the raw commit: `cat-file` is plumbing, so
/// `log.showSignature` and the like do not reach its output.
fn read_commit(git: &Git, commit: &Commit) -> Result<(Tree, SystemTime), SourceError> {
    let out = git.run(&["cat-file", "commit", &commit.0])?;
    let text = String::from_utf8_lossy(&out);
    let mut tree = None;
    let mut seconds = None;
    for line in text.split('\n').take_while(|line| !line.is_empty()) {
        if let Some(oid) = line.strip_prefix("tree ") {
            tree = Some(Tree(oid.to_owned()));
        } else if let Some(committer) = line.strip_prefix("committer ") {
            // `<name> <<email>> <seconds> <zone>`
            seconds = committer
                .rsplit(' ')
                .nth(1)
                .and_then(|seconds| seconds.parse().ok());
        }
    }
    match (tree, seconds) {
        (Some(tree), Some(seconds)) => Ok((tree, UNIX_EPOCH + Duration::from_secs(seconds))),
        _ => Err(unexpected("cat-file commit")),
    }
}

fn unexpected(command: &str) -> SourceError {
    SourceError::Git(GitError::Io(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("unexpected output of `git {command}`"),
    )))
}

fn utf8(path: PathBuf) -> Result<Utf8PathBuf, SourceError> {
    Utf8PathBuf::from_path_buf(path).map_err(|path| {
        let source = io::Error::new(io::ErrorKind::InvalidData, "the path is not UTF-8");
        SourceError::io(&path)(source)
    })
}

fn text(stdout: Vec<u8>) -> String {
    String::from_utf8_lossy(&stdout).trim_end().to_owned()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
