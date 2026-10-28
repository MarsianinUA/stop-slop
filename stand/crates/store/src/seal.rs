//! The sealed copy of the recipe, the journal of a session and its acceptances. `store` copies
//! the folder; `spec` compares it.

use std::fs;
use std::io::{self, ErrorKind};
use std::time::SystemTime;

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::session::Session;
use crate::{StoreError, entries, read_json, remove, write_json};

const SEAL: &str = "seal";
const ACCEPTED: &str = "accepted.json";

/// `journal` is the whole journal of the session, `clean` included.
#[derive(Debug)]
pub struct Sealed {
    pub dir: Utf8PathBuf,
    pub sealed_at: SystemTime,
    pub journal: Vec<JournalEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged, rename_all_fields = "camelCase")]
pub enum JournalEntry {
    Sealed {
        #[serde(with = "crate::rfc3339")]
        sealed_at: SystemTime,
        digest: String,
    },
    Resealed {
        #[serde(with = "crate::rfc3339")]
        resealed_at: SystemTime,
        digest: String,
        reason: String,
    },
    Cleaned {
        #[serde(with = "crate::rfc3339")]
        cleaned_at: SystemTime,
    },
}

/// `allow` is the text `Change::parse` accepted.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Acceptance {
    pub measure: String,
    pub allow: String,
    pub reason: String,
    pub head_tree: String,
    #[serde(with = "crate::rfc3339")]
    pub accepted_at: SystemTime,
}

impl Session {
    /// The copy, when the last journal entry is a seal or a reseal, with its time.
    pub fn sealed(&self) -> Result<Option<Sealed>, StoreError> {
        let dir = self.dir.join(SEAL);
        if !dir.try_exists().map_err(StoreError::io(&dir))? {
            return Ok(None);
        }
        let journal = self.journal()?;
        let sealed_at = journal.last().and_then(|entry| match entry {
            JournalEntry::Sealed { sealed_at, .. } => Some(*sealed_at),
            JournalEntry::Resealed { resealed_at, .. } => Some(*resealed_at),
            JournalEntry::Cleaned { .. } => None,
        });
        Ok(sealed_at.map(|sealed_at| Sealed {
            dir,
            sealed_at,
            journal,
        }))
    }

    /// Copies the files of `stand_dir` as the sealed recipe and appends `{sealedAt, digest}` to
    /// the journal.
    pub fn seal(
        &self,
        stand_dir: &Utf8Path,
        digest: &str,
        now: SystemTime,
    ) -> Result<Sealed, StoreError> {
        let entry = JournalEntry::Sealed {
            sealed_at: now,
            digest: digest.to_owned(),
        };
        self.replace_copy(stand_dir, now, entry)
    }

    /// Replaces the copy and appends `{resealedAt, digest, reason}` to the journal.
    pub fn reseal(
        &self,
        stand_dir: &Utf8Path,
        digest: &str,
        now: SystemTime,
        reason: &str,
    ) -> Result<Sealed, StoreError> {
        let entry = JournalEntry::Resealed {
            resealed_at: now,
            digest: digest.to_owned(),
            reason: reason.to_owned(),
        };
        self.replace_copy(stand_dir, now, entry)
    }

    /// Appends; earlier acceptances of the same measure stay.
    pub fn accept(&self, acceptance: Acceptance) -> Result<(), StoreError> {
        let mut acceptances = self.acceptances()?;
        acceptances.push(acceptance);
        write_json(&self.dir.join(ACCEPTED), &acceptances)
    }

    /// Oldest first.
    pub fn acceptances(&self) -> Result<Vec<Acceptance>, StoreError> {
        Ok(read_json(&self.dir.join(ACCEPTED))?.unwrap_or_default())
    }

    /// The journal gets the entry before the copy changes: a crash between the two leaves an
    /// entry for a copy that did not change, never a change without an entry. The copy is whole
    /// before it takes the place of the old one, and the old one moves aside before it is
    /// deleted: deleting it in place can stop halfway on a file another process holds open
    /// (Windows), which would leave half a copy in `seal/`.
    fn replace_copy(
        &self,
        stand_dir: &Utf8Path,
        now: SystemTime,
        entry: JournalEntry,
    ) -> Result<Sealed, StoreError> {
        let dir = self.dir.join(SEAL);
        let (temp, old) = (self.dir.join("seal.tmp"), self.dir.join("seal.old"));
        remove(&temp)?;
        remove(&old)?;
        copy_folder(stand_dir, &temp)?;
        let journal = append(&self.journal, entry)?;
        match fs::rename(&dir, &old) {
            Err(error) if error.kind() != ErrorKind::NotFound => {
                return Err(StoreError::io(&dir)(error));
            }
            _ => {}
        }
        fs::rename(&temp, &dir).map_err(StoreError::io(&dir))?;
        // The seal is done; a copy left aside goes with the next one.
        let _ = remove(&old);
        Ok(Sealed {
            dir,
            sealed_at: now,
            journal,
        })
    }

    fn journal(&self) -> Result<Vec<JournalEntry>, StoreError> {
        Ok(read_json(&self.journal)?.unwrap_or_default())
    }
}

/// `{cleanedAt}` at the end of every journal in `dir`.
pub(crate) fn append_cleaned(dir: &Utf8Path, now: SystemTime) -> Result<(), StoreError> {
    for entry in entries(dir)? {
        if entry.path().extension() == Some("json") {
            append_cleaned_to(entry.path(), now)?;
        }
    }
    Ok(())
}

/// `{cleanedAt}` at the end of the journal at `path`. Not when there is none: a session that never
/// sealed has none, and a journal that starts with a clean tells nothing. Not after another clean:
/// nothing was sealed since, and a `forget` that failed after the entry and runs again adds no second.
pub(crate) fn append_cleaned_to(path: &Utf8Path, now: SystemTime) -> Result<(), StoreError> {
    let Some(mut journal) = read_json::<Vec<JournalEntry>>(path)? else {
        return Ok(());
    };
    if !matches!(journal.last(), Some(JournalEntry::Cleaned { .. })) {
        journal.push(JournalEntry::Cleaned { cleaned_at: now });
        write_json(path, &journal)?;
    }
    Ok(())
}

fn append(path: &Utf8Path, entry: JournalEntry) -> Result<Vec<JournalEntry>, StoreError> {
    let mut journal: Vec<JournalEntry> = read_json(path)?.unwrap_or_default();
    journal.push(entry);
    let dir = path
        .parent()
        .expect("a journal is in the folder of journals");
    fs::create_dir_all(dir).map_err(StoreError::io(dir))?;
    write_json(path, &journal)?;
    Ok(journal)
}

/// Every folder, empty ones too (`spec` refuses a bind source that is not there), every file, and
/// a link as a link to the same path: the recipe counts a link by the path it holds.
fn copy_folder(from: &Utf8Path, to: &Utf8Path) -> Result<(), StoreError> {
    fs::create_dir(to).map_err(StoreError::io(to))?;
    for entry in from.read_dir_utf8().map_err(StoreError::io(from))? {
        let entry = entry.map_err(StoreError::io(from))?;
        let (path, target) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type().map_err(StoreError::io(path))?;
        if kind.is_dir() {
            copy_folder(path, &target)?;
        } else if kind.is_symlink() {
            copy_link(path, &target, kind).map_err(StoreError::io(path))?;
        } else if kind.is_file() {
            fs::copy(path, &target).map_err(StoreError::io(path))?;
        } else {
            return Err(StoreError::io(path)(io::Error::new(
                ErrorKind::InvalidInput,
                "neither a file, a folder nor a link",
            )));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn copy_link(from: &Utf8Path, to: &Utf8Path, _kind: fs::FileType) -> io::Result<()> {
    std::os::unix::fs::symlink(fs::read_link(from)?, to)
}

/// Making a link on Windows takes the privilege to make links (developer mode or an admin).
#[cfg(windows)]
fn copy_link(from: &Utf8Path, to: &Utf8Path, kind: fs::FileType) -> io::Result<()> {
    use std::os::windows::fs::{FileTypeExt, symlink_dir, symlink_file};
    let held = fs::read_link(from)?;
    if kind.is_symlink_dir() {
        symlink_dir(held, to)
    } else {
        symlink_file(held, to)
    }
}
