use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub disabled: Vec<String>,
    /// `[rules.<id>]`. The ids and the keys are checked by `Engine::new`, not here.
    pub rules: BTreeMap<String, toml::Table>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IgnoreEntry {
    pub fingerprint: String,
    pub rule: String,
    pub file: Utf8PathBuf,
    pub reason: String,
    /// `YYYY-MM-DD`.
    pub added: String,
}

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("{}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// `line` is from 1; `message` has no position.
    #[error("{}:{line}: {message}", .path.display())]
    Parse {
        path: PathBuf,
        line: u32,
        message: String,
    },
}

impl StateError {
    fn io(path: &Path) -> impl Fn(io::Error) -> StateError {
        move |source| StateError::Io {
            path: path.to_owned(),
            source,
        }
    }
}

#[derive(Debug)]
pub struct State {
    dir: PathBuf,
}

impl State {
    /// The common dir, not `.git`: in a linked worktree `.git` is a file, and the marker,
    /// `config.toml` and `ignore.json` are one for all worktrees of the repo.
    pub fn open(git_common_dir: &Path) -> State {
        State {
            dir: git_common_dir.join("info").join("stop-slop"),
        }
    }

    /// An error of the file system reads as off: without the marker the hook does nothing.
    pub fn is_enabled(&self) -> bool {
        self.dir.join("enabled").is_file()
    }

    pub fn enable(&self) -> Result<(), StateError> {
        // `info/` is missing after `git init` with an empty template and in a submodule.
        fs::create_dir_all(&self.dir).map_err(StateError::io(&self.dir))?;
        let path = self.dir.join("enabled");
        File::create(&path).map_err(StateError::io(&path))?;
        Ok(())
    }

    pub fn disable(&self) -> Result<(), StateError> {
        let path = self.dir.join("enabled");
        match fs::remove_file(&path) {
            Err(source) if source.kind() != ErrorKind::NotFound => {
                Err(StateError::Io { path, source })
            }
            _ => Ok(()),
        }
    }

    /// For a message about the file: `Engine::new` checks the ids and keys of `config.toml`
    /// without knowing where it lies.
    pub fn config_path(&self) -> PathBuf {
        self.dir.join("config.toml")
    }

    pub fn config(&self) -> Result<Config, StateError> {
        let path = self.config_path();
        let Some(text) = read(&path)? else {
            return Ok(Config::default());
        };
        toml::from_str(&text).map_err(|error| toml_error(path, &text, &error))
    }

    pub fn ignores(&self) -> Result<Vec<IgnoreEntry>, StateError> {
        let path = self.dir.join("ignore.json");
        let Some(text) = read(&path)? else {
            return Ok(Vec::new());
        };
        // The file is edited by hand, and Notepad may save it with a BOM, which `serde_json`
        // rejects. `toml` takes a BOM itself.
        let json = text.strip_prefix('\u{feff}').unwrap_or(&text);
        serde_json::from_str(json).map_err(|error| json_error(path, &error))
    }

    /// An entry with the `file` and `fingerprint` of an old one replaces it in place; others
    /// go to the end. A broken `ignore.json` is left as it is.
    pub fn add_ignore(&self, entries: Vec<IgnoreEntry>) -> Result<(), StateError> {
        fs::create_dir_all(&self.dir).map_err(StateError::io(&self.dir))?;
        // Two `ignore` at once would read the same list, and the second rename would drop the
        // entry of the first. The lock is a file of its own: the rename replaces `ignore.json`,
        // so a writer waiting on it would hold a lock on the old file. The OS releases the lock
        // when the process dies.
        let lock_path = self.dir.join("ignore.json.lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(StateError::io(&lock_path))?;
        lock.lock().map_err(StateError::io(&lock_path))?;

        let mut list = self.ignores()?;
        for entry in entries {
            let old = list
                .iter_mut()
                .find(|old| old.file == entry.file && old.fingerprint == entry.fingerprint);
            match old {
                Some(old) => *old = entry,
                None => list.push(entry),
            }
        }
        let mut text = serde_json::to_string_pretty(&list).expect("a list of strings serializes");
        text.push('\n');
        write_atomic(&self.dir.join("ignore.json"), text.as_bytes())
    }
}

/// `None` when the file or its folder is missing.
fn read(path: &Path) -> Result<Option<String>, StateError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(source) => Err(StateError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

/// A reader sees the old file or the new one whole. The temp has a fixed name, since writers
/// take turns under the lock; a temp left by a crash is overwritten by the next call.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StateError> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    let io_error = StateError::io(&temp);
    let mut file = File::create(&temp).map_err(&io_error)?;
    file.write_all(bytes).map_err(&io_error)?;
    // Without it a power cut after the rename may leave an empty file, and `ignore.json` is
    // the only copy of the decisions. The folder is not synced: a crash may lose the last
    // write, but not break the file.
    file.sync_all().map_err(&io_error)?;
    drop(file);
    fs::rename(&temp, path).map_err(StateError::io(path))
}

fn toml_error(path: PathBuf, text: &str, error: &toml::de::Error) -> StateError {
    // Bytes: slicing the `str` would panic on a span that starts inside a character.
    let line = error.span().map_or(1, |span| {
        text.as_bytes()[..span.start]
            .iter()
            .filter(|&&byte| byte == b'\n')
            .count()
            + 1
    });
    StateError::Parse {
        path,
        line: to_u32(line),
        message: error.message().to_owned(),
    }
}

fn json_error(path: PathBuf, error: &serde_json::Error) -> StateError {
    let mut message = error.to_string();
    let position = format!(" at line {} column {}", error.line(), error.column());
    if message.ends_with(&position) {
        message.truncate(message.len() - position.len());
    }
    StateError::Parse {
        path,
        line: to_u32(error.line()),
        message,
    }
}

fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}
