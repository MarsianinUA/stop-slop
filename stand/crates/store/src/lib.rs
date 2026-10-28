//! The stand's folder. Under `root`:
//!
//! ```text
//! lock                            the run lock; never deleted
//! msb/                            the sandbox home
//! trees/{head,before}.tar         the trees of a run
//! journal/<key>.json              the journal of a session; outlives `clean`
//! sessions/<key>/session.json     a session: one stand file in one worktree
//!                seal/            the sealed copy of the recipe
//!                accepted.json
//!                runs/<id>/run.json
//!                cache/<digest>/  an entry of the setup cache
//! ```

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, ErrorKind, Write};
use std::time::SystemTime;

use camino::{Utf8DirEntry, Utf8Path, Utf8PathBuf};
use serde::Serialize;
use serde::de::DeserializeOwned;

mod cache;
mod seal;
mod session;

pub use cache::{CacheEntry, SetupKey, Slot};
pub use seal::{Acceptance, JournalEntry, Sealed};
pub use session::{PoolKey, RunPaths, RunRecord, Session};

const LOCK: &str = "lock";
const TREES: &str = "trees";
const SESSIONS: &str = "sessions";
const JOURNAL: &str = "journal";

/// The key of a `ConfigError` that is about the whole file: broken TOML or a file that cannot be
/// read.
const WHOLE_FILE: &str = "toml";

pub struct StorePaths {
    pub root: Utf8PathBuf,
    /// `stand.toml`; on macOS the config and data folders are one, so it lies in `root`.
    pub config: Utf8PathBuf,
}

pub struct Store {
    paths: StorePaths,
}

impl Store {
    pub fn open(paths: StorePaths) -> Result<Store, StoreError> {
        fs::create_dir_all(&paths.root).map_err(StoreError::io(&paths.root))?;
        Ok(Store { paths })
    }

    /// `None` while another process holds the OS lock on `<root>/lock`.
    pub fn try_lock(&self) -> Result<Option<RunLock>, StoreError> {
        let path = self.paths.root.join(LOCK);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(StoreError::io(&path))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(RunLock { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(source)) => Err(StoreError::Io { path, source }),
        }
    }

    pub fn sandbox_home(&self) -> Utf8PathBuf {
        self.paths.root.join("msb")
    }

    /// An unknown key, a wrong type or broken TOML: exit 2, as `config.toml` of the linter.
    pub fn config(&self) -> Result<UserConfig, ConfigError> {
        let file = &self.paths.config;
        let error = |key: &str, message: String| ConfigError {
            file: file.clone(),
            key: key.to_owned(),
            message,
        };
        let text = match fs::read_to_string(file) {
            Ok(text) => text,
            Err(source) if source.kind() == ErrorKind::NotFound => {
                return Ok(UserConfig::default());
            }
            Err(source) => return Err(error(WHOLE_FILE, source.to_string())),
        };
        let table: toml::Table = toml::from_str(&text)
            .map_err(|syntax: toml::de::Error| error(WHOLE_FILE, toml_message(&text, &syntax)))?;
        let mut config = UserConfig::default();
        for (key, value) in &table {
            let wrong = |message| error(key, message);
            match key.as_str() {
                "host_share" => config.host_share = Some(whole(value, "0 to 255").map_err(wrong)?),
                "max_tolerance" => {
                    config.max_tolerance = Some(whole(value, "0 to 65535").map_err(wrong)?);
                }
                _ => {
                    let known = "unknown key; stand.toml takes host_share and max_tolerance";
                    return Err(wrong(known.to_owned()));
                }
            }
        }
        Ok(config)
    }

    pub fn trees(&self) -> Result<Trees, StoreError> {
        let dir = self.paths.root.join(TREES);
        fs::create_dir_all(&dir).map_err(StoreError::io(&dir))?;
        Ok(Trees {
            head: dir.join("head.tar"),
            before: dir.join("before.tar"),
        })
    }

    pub fn clear_trees(&self) -> Result<(), StoreError> {
        remove(&self.paths.root.join(TREES))
    }

    /// `Sessions` also appends `{cleanedAt}` to the journal of every session. `All` leaves only
    /// `lock`, and `stand.toml` when it lies in `root`.
    pub fn clean(&self, what: Clean) -> Result<(), StoreError> {
        let root = &self.paths.root;
        match what {
            Clean::Sessions => {
                seal::append_cleaned(&root.join(JOURNAL), SystemTime::now())?;
                remove(&root.join(SESSIONS))?;
                self.clear_trees()
            }
            Clean::All => {
                for entry in root.read_dir_utf8().map_err(StoreError::io(root))? {
                    let entry = entry.map_err(StoreError::io(root))?;
                    if entry.file_name() != LOCK && entry.path() != self.paths.config {
                        remove(entry.path())?;
                    }
                }
                Ok(())
            }
        }
    }
}

/// The value of a key that holds a whole number of `T`; the range is for the message.
fn whole<T: TryFrom<i64>>(value: &toml::Value, range: &str) -> Result<T, String> {
    value
        .as_integer()
        .and_then(|number| T::try_from(number).ok())
        .ok_or_else(|| format!("expected a whole number from {range}, found {value}"))
}

fn toml_message(text: &str, error: &toml::de::Error) -> String {
    // Bytes: slicing the `str` would panic on a span that starts inside a character.
    let line = error.span().map_or(1, |span| {
        text.as_bytes()[..span.start]
            .iter()
            .filter(|&&byte| byte == b'\n')
            .count()
            + 1
    });
    format!("line {line}: {}", error.message().trim_end())
}

/// Released on drop; the file stays.
pub struct RunLock {
    _file: File,
}

#[derive(Debug, Default)]
pub struct UserConfig {
    pub host_share: Option<u8>,
    pub max_tolerance: Option<u16>,
}

/// The one error of `stand.toml`; `cli` also reports `BudgetError::Share` and `spec::PolicyError`
/// with it.
#[derive(Debug, thiserror::Error)]
#[error("{file}: {key}: {message}")]
pub struct ConfigError {
    pub file: Utf8PathBuf,
    pub key: String,
    pub message: String,
}

/// The paths of the tar files of both trees; `clear_trees` deletes them.
#[derive(Debug)]
pub struct Trees {
    pub head: Utf8PathBuf,
    pub before: Utf8PathBuf,
}

#[derive(Debug)]
pub struct Expired {
    key: String,
    at: SystemTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clean {
    Sessions,
    All,
}

/// `OtherThan` keeps the kept run, or the cache, of the session with this key (`Session::key()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeptScope {
    OtherThan(String),
    All,
}

impl KeptScope {
    fn covers(&self, key: &str) -> bool {
        match self {
            KeptScope::OtherThan(own) => own != key,
            KeptScope::All => true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{path}: {source}")]
    Io {
        path: Utf8PathBuf,
        #[source]
        source: io::Error,
    },
}

impl StoreError {
    fn io(path: &Utf8Path) -> impl Fn(io::Error) -> StoreError {
        let path = path.to_owned();
        move |source| StoreError::Io {
            path: path.clone(),
            source,
        }
    }
}

/// `None` when the file is missing. A file the stand wrote that does not parse is `InvalidData`.
fn read_json<T: DeserializeOwned>(path: &Utf8Path) -> Result<Option<T>, StoreError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(source) if source.kind() == ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(StoreError::io(path)(source)),
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| StoreError::io(path)(io::Error::new(ErrorKind::InvalidData, error)))
}

/// A reader sees the old file or the new one whole. The temp has a fixed name: writers take turns
/// under the run lock, and a temp a crash left is overwritten by the next write.
fn write_json<T: Serialize>(path: &Utf8Path, value: &T) -> Result<(), StoreError> {
    let temp = Utf8PathBuf::from(format!("{path}.tmp"));
    let mut text = serde_json::to_string_pretty(value).expect("the stand's records serialize");
    text.push('\n');
    let io_error = StoreError::io(&temp);
    let mut file = File::create(&temp).map_err(&io_error)?;
    file.write_all(text.as_bytes()).map_err(&io_error)?;
    file.sync_all().map_err(&io_error)?;
    drop(file);
    fs::rename(&temp, path).map_err(StoreError::io(path))
}

fn entries(dir: &Utf8Path) -> Result<Vec<Utf8DirEntry>, StoreError> {
    match dir.read_dir_utf8() {
        Ok(entries) => entries
            .collect::<io::Result<_>>()
            .map_err(StoreError::io(dir)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(StoreError::io(dir)(error)),
    }
}

/// A folder with all in it, or a file; nothing there is not an error.
fn remove(path: &Utf8Path) -> Result<(), StoreError> {
    let removed = fs::symlink_metadata(path).and_then(|meta| {
        if meta.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        }
    });
    match removed {
        Err(error) if error.kind() != ErrorKind::NotFound => Err(StoreError::io(path)(error)),
        _ => Ok(()),
    }
}

/// Times in the JSON files: RFC 3339 in UTC, to the nanosecond when there is a fraction.
mod rfc3339 {
    use std::time::SystemTime;

    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(time: &SystemTime, out: S) -> Result<S::Ok, S::Error> {
        out.collect_str(&humantime::format_rfc3339(*time))
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(input: D) -> Result<SystemTime, D::Error> {
        let text = String::deserialize(input)?;
        humantime::parse_rfc3339(&text).map_err(serde::de::Error::custom)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
