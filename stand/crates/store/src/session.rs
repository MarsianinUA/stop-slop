//! Sessions: one per stand file and worktree, with its runs, the kept run and the time it was
//! last used, which the TTL counts from.

use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::time::{Duration, SystemTime};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    Expired, JOURNAL, KeptScope, SESSIONS, Store, StoreError, hex, read_json, remove, write_json,
};

/// Of a session and of what it keeps: its runs, the kept run's snapshots, its setup cache.
const TTL: Duration = Duration::from_secs(3 * 24 * 60 * 60);

const SESSION: &str = "session.json";
const RUNS: &str = "runs";
const RUN: &str = "run.json";

pub struct Session {
    key: String,
    pub(crate) dir: Utf8PathBuf,
    /// `<root>/journal/<key>.json`, outside `sessions/`: it outlives `clean`.
    pub(crate) journal: Utf8PathBuf,
}

/// `session.json`.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionFile {
    file: Utf8PathBuf,
    worktree: Utf8PathBuf,
    images: Vec<String>,
    #[serde(with = "crate::rfc3339")]
    used_at: SystemTime,
    /// The id of the kept run.
    kept: Option<String>,
}

/// `run.json`.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunFile {
    key: PoolKey,
    #[serde(with = "crate::rfc3339")]
    created_at: SystemTime,
    passed: bool,
}

/// Built only by `store`; `id` is `YYYYMMDD-HHMMSS`. The file names inside `dir` belong to their
/// writers (`run`, `report`).
#[derive(Debug, Clone)]
pub struct RunPaths {
    pub id: String,
    pub dir: Utf8PathBuf,
}

/// Runs with one key pool their repeats.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolKey {
    pub head_tree: String,
    pub before_tree: Option<String>,
    pub digest: String,
}

#[derive(Debug, Clone)]
pub struct RunRecord {
    pub id: String,
    pub created_at: SystemTime,
    pub key: PoolKey,
    pub passed: bool,
}

impl Store {
    /// Creates the session or marks it used at `now`; `images` are the images of its stand.
    pub fn session(
        &self,
        file: &Utf8Path,
        worktree: &Utf8Path,
        images: &[String],
        now: SystemTime,
    ) -> Result<Session, StoreError> {
        let canonical = |path: &Utf8Path| path.canonicalize_utf8().map_err(StoreError::io(path));
        let (file, worktree) = (canonical(file)?, canonical(worktree)?);
        let hash = Sha256::digest(format!("{file}\n{worktree}"));
        let session = self.session_of(&hex(&hash)[..16]);
        fs::create_dir_all(&session.dir).map_err(StoreError::io(&session.dir))?;
        let images = images.to_vec();
        let state = match session.read()? {
            Some(old) => SessionFile {
                file,
                worktree,
                images,
                used_at: now,
                ..old
            },
            None => SessionFile {
                file,
                worktree,
                images,
                used_at: now,
                kept: None,
            },
        };
        session.write(&state)?;
        Ok(session)
    }

    pub fn expired(&self, now: SystemTime) -> Result<Vec<Expired>, StoreError> {
        Ok(self
            .sessions()?
            .into_iter()
            .filter(|(_, state)| is_expired(state, now))
            .map(|(session, _)| Expired { key: session.key })
            .collect())
    }

    /// Their sessions, runs, reports, kept runs and setup cache; their journals stay.
    pub fn forget(&self, expired: &[Expired]) -> Result<(), StoreError> {
        expired
            .iter()
            .try_for_each(|expired| remove(&self.session_of(&expired.key).dir))
    }

    /// Images of every session, which after `forget` are the ones not expired: a snapshot fixes
    /// its image.
    pub fn kept_images(&self) -> Result<Vec<String>, StoreError> {
        let mut images = BTreeSet::new();
        for (_, state) in self.sessions()? {
            images.extend(state.images);
        }
        Ok(images.into_iter().collect())
    }

    /// The kept run of every session; the sweep leaves the snapshots `run::kept_ids` reads from
    /// them.
    pub fn kept_runs(&self) -> Result<Vec<RunPaths>, StoreError> {
        Ok(self
            .sessions()?
            .into_iter()
            .filter_map(|(session, state)| Some(session.run(&state.kept?)))
            .collect())
    }

    /// Forgets the kept runs of other sessions, or of all, and returns them for `run::kept_ids`
    /// and `drop_snapshot`.
    pub fn drop_kept(&self, scope: KeptScope) -> Result<Vec<RunPaths>, StoreError> {
        let mut dropped = Vec::new();
        for (session, mut state) in self.sessions()? {
            if !scope.covers(&session.key) {
                continue;
            }
            if let Some(kept) = state.kept.take() {
                session.write(&state)?;
                dropped.push(session.run(&kept));
            }
        }
        Ok(dropped)
    }

    pub(crate) fn session_of(&self, key: &str) -> Session {
        let root = &self.paths.root;
        Session {
            key: key.to_owned(),
            dir: root.join(SESSIONS).join(key),
            journal: root.join(JOURNAL).join(format!("{key}.json")),
        }
    }

    /// Every session with its `session.json`; a folder without one (a crash between the two) is
    /// not a session.
    pub(crate) fn sessions(&self) -> Result<Vec<(Session, SessionFile)>, StoreError> {
        let dir = self.paths.root.join(SESSIONS);
        let mut sessions = Vec::new();
        for name in folders(&dir)? {
            let session = self.session_of(&name);
            if let Some(state) = session.read()? {
                sessions.push((session, state));
            }
        }
        Ok(sessions)
    }
}

fn is_expired(state: &SessionFile, now: SystemTime) -> bool {
    now.duration_since(state.used_at)
        .is_ok_and(|age| age >= TTL)
}

impl Session {
    /// The session key: a hash of the stand file path and the worktree root.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Creates the run folder and writes `run.json` with `key`. A second run in one second takes
    /// the next free one.
    pub fn new_run(&self, now: SystemTime, key: &PoolKey) -> Result<RunPaths, StoreError> {
        let runs = self.dir.join(RUNS);
        fs::create_dir_all(&runs).map_err(StoreError::io(&runs))?;
        let mut at = now;
        let run = loop {
            let run = self.run(&run_id(at));
            match fs::create_dir(&run.dir) {
                Ok(()) => break run,
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    at += Duration::from_secs(1);
                }
                Err(error) => return Err(StoreError::io(&run.dir)(error)),
            }
        };
        let record = RunFile {
            key: key.clone(),
            created_at: now,
            passed: false,
        };
        write_json(&run.dir.join(RUN), &record)?;
        Ok(run)
    }

    /// After `Report::write`: `pool` and `previous_report` see only marked runs.
    pub fn passed(&self, run: &RunPaths) -> Result<(), StoreError> {
        let path = run.dir.join(RUN);
        let mut record: RunFile = read_json(&path)?.ok_or_else(|| missing(&path))?;
        record.passed = true;
        write_json(&path, &record)
    }

    /// Passed runs with `key`, oldest first.
    pub fn pool(&self, key: &PoolKey) -> Result<Vec<RunPaths>, StoreError> {
        Ok(self
            .history()?
            .into_iter()
            .filter(|record| record.passed && record.key == *key)
            .map(|record| self.run(&record.id))
            .collect())
    }

    /// The last passed run whose key differs from `key`.
    pub fn previous_report(&self, key: &PoolKey) -> Result<Option<RunPaths>, StoreError> {
        Ok(self
            .history()?
            .into_iter()
            .rev()
            .find(|record| record.passed && record.key != *key)
            .map(|record| self.run(&record.id)))
    }

    /// Every run of the session, oldest first. A folder without `run.json` (a crash between the
    /// two) is not a run.
    pub fn history(&self) -> Result<Vec<RunRecord>, StoreError> {
        let mut records = Vec::new();
        for id in folders(&self.dir.join(RUNS))? {
            let run = self.run(&id);
            if let Some(record) = read_json::<RunFile>(&run.dir.join(RUN))? {
                records.push(RunRecord {
                    id,
                    created_at: record.created_at,
                    key: record.key,
                    passed: record.passed,
                });
            }
        }
        Ok(records)
    }

    /// Records `run` as the kept run of the session; returns the run kept before, whose snapshots
    /// `cli` drops.
    pub fn keep(&self, run: &RunPaths) -> Result<Option<RunPaths>, StoreError> {
        let mut state = self.state()?;
        let before = state.kept.replace(run.id.clone());
        self.write(&state)?;
        Ok(before
            .filter(|before| *before != run.id)
            .map(|before| self.run(&before)))
    }

    /// The kept run: the last run that wrote a `kept.json`; `inspect` takes it.
    pub fn kept(&self) -> Result<Option<RunPaths>, StoreError> {
        Ok(self.state()?.kept.map(|id| self.run(&id)))
    }

    fn run(&self, id: &str) -> RunPaths {
        RunPaths {
            id: id.to_owned(),
            dir: self.dir.join(RUNS).join(id),
        }
    }

    pub(crate) fn read(&self) -> Result<Option<SessionFile>, StoreError> {
        read_json(&self.dir.join(SESSION))
    }

    pub(crate) fn state(&self) -> Result<SessionFile, StoreError> {
        let path = self.dir.join(SESSION);
        read_json(&path)?.ok_or_else(|| missing(&path))
    }

    pub(crate) fn write(&self, state: &SessionFile) -> Result<(), StoreError> {
        write_json(&self.dir.join(SESSION), state)
    }
}

/// `YYYYMMDD-HHMMSS` in UTC.
fn run_id(at: SystemTime) -> String {
    let time = humantime::format_rfc3339_seconds(at).to_string();
    let digits = |from: usize, to: usize| -> String {
        time[from..to]
            .chars()
            .filter(char::is_ascii_digit)
            .collect()
    };
    format!("{}-{}", digits(0, 10), digits(11, 19))
}

/// The names of the folders in `dir`, sorted; none when `dir` is missing.
pub(crate) fn folders(dir: &Utf8Path) -> Result<Vec<String>, StoreError> {
    let entries = match dir.read_dir_utf8() {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(StoreError::io(dir)(error)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(StoreError::io(dir))?;
        if entry
            .file_type()
            .map_err(StoreError::io(entry.path()))?
            .is_dir()
        {
            names.push(entry.file_name().to_owned());
        }
    }
    names.sort();
    Ok(names)
}

fn missing(path: &Utf8Path) -> StoreError {
    StoreError::io(path)(std::io::Error::from(ErrorKind::NotFound))
}
