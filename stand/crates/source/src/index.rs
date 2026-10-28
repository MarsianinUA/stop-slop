use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU32, Ordering};

use gitcmd::Git;

use crate::{SourceError, text};

/// Tells apart the indexes of one process: snapshots may run in parallel threads.
static NEXT: AtomicU32 = AtomicU32::new(0);

/// A copy of the work tree's index in the OS temp folder, for `GIT_INDEX_FILE`: git writes the
/// copy, and the work tree's own index stays as it was. Removed on drop.
///
/// The copy keeps the stat cache, so only changed files are hashed again, and the entries' modes:
/// under `core.symlinks=false` and `core.fileMode=false`, as on Windows, a link is a plain file
/// on disk and the executable bit is not there, and `git add` keeps both from the index.
pub(crate) struct TempIndex {
    path: PathBuf,
}

impl TempIndex {
    pub(crate) fn copy_of(git: &Git) -> Result<TempIndex, SourceError> {
        // In a linked worktree `.git` is a file, and the index lies in `<common>/worktrees/<name>/`.
        let out = git.run(&["rev-parse", "--path-format=absolute", "--git-path", "index"])?;
        let index = PathBuf::from(text(out));
        let name = format!(
            "stop-slop-stand-index-{}-{}",
            process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let temp = TempIndex {
            path: env::temp_dir().join(name),
        };
        // Left by a git killed while it wrote an index of this name, it would stop every `add`.
        remove(&temp.lock())?;
        match fs::read(&index) {
            Ok(bytes) => fs::write(&temp.path, bytes).map_err(SourceError::io(&temp.path))?,
            // Nothing was ever added: git starts an index of its own.
            Err(error) if error.kind() == ErrorKind::NotFound => remove(&temp.path)?,
            Err(source) => return Err(SourceError::io(&index)(source)),
        }
        Ok(temp)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> PathBuf {
        let mut lock = self.path.clone().into_os_string();
        lock.push(".lock");
        lock.into()
    }
}

impl Drop for TempIndex {
    /// The lock is left when Ctrl+C stopped git and not this process.
    fn drop(&mut self) {
        // A file left in the temp folder costs only its bytes; a drop has no one to tell.
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(self.lock());
    }
}

fn remove(path: &Path) -> Result<(), SourceError> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != ErrorKind::NotFound => Err(SourceError::io(path)(error)),
        _ => Ok(()),
    }
}
