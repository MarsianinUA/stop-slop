use std::env;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

use gitcmd::Git;
use tempfile::TempDir;

use crate::{SourceError, text};

/// A copy of the work tree's index for `GIT_INDEX_FILE`, at `index` in a temp folder of its own:
/// git writes the copy and its lock there, and the work tree's own index stays as it was. The
/// folder goes on drop.
///
/// The copy keeps the stat cache, so only changed files are hashed again, and the entries' modes:
/// under `core.symlinks=false` and `core.fileMode=false`, as on Windows, a link is a plain file
/// on disk and the executable bit is not there, and `git add` keeps both from the index.
pub(crate) struct TempIndex {
    _dir: TempDir,
    path: PathBuf,
}

impl TempIndex {
    pub(crate) fn copy_of(git: &Git) -> Result<TempIndex, SourceError> {
        // In a linked worktree `.git` is a file, and the index lies in `<common>/worktrees/<name>/`.
        let out = git.run(&["rev-parse", "--path-format=absolute", "--git-path", "index"])?;
        let index = PathBuf::from(text(out));
        let mut folder = tempfile::Builder::new();
        folder.prefix("stop-slop-stand-index-");
        // A temp folder is world-readable by default, and the index lists every path of the repo.
        #[cfg(unix)]
        folder.permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700));
        let dir = folder
            .tempdir()
            .map_err(SourceError::io(&env::temp_dir()))?;
        let temp = TempIndex {
            path: dir.path().join("index"),
            _dir: dir,
        };
        let mut original = match File::open(&index) {
            Ok(original) => original,
            // Nothing was ever added: git starts an index of its own.
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(temp),
            Err(source) => return Err(SourceError::io(&index)(source)),
        };
        let mut bytes = Vec::new();
        let modified = original
            .read_to_end(&mut bytes)
            .and_then(|_| original.metadata()?.modified())
            .map_err(SourceError::io(&index))?;
        let mut copy = File::create(&temp.path).map_err(SourceError::io(&temp.path))?;
        // Git hashes a file again when its entry is dated no earlier than the index file, as the
        // file may have changed in the second the index was written. A copy dated now would take
        // such a file for unchanged.
        copy.write_all(&bytes)
            .and_then(|()| copy.set_modified(modified))
            .map_err(SourceError::io(&temp.path))?;
        Ok(temp)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}
