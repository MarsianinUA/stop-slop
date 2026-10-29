//! One run at a time per stand folder (plan, row 21): a second command waits for the lock as
//! long as a whole run may take, `run::span` of its stand file.

use std::io::Write;
use std::thread;
use std::time::{Duration, Instant};

use run::Cancel;
use spec::Stand;
use store::{RunLock, Store};

use crate::error::StandError;

const POLL: Duration = Duration::from_secs(1);

/// `run::span` of the stand file, or `run::DEFAULT_SPAN` for a command whose stand file does not
/// read (Gate 1, «Предел без файла стенда»).
pub(crate) fn limit(stand: Option<&Stand>) -> Duration {
    stand.map_or(run::DEFAULT_SPAN, run::span)
}

pub(crate) fn wait(
    store: &Store,
    limit: Duration,
    cancel: &Cancel,
    err: &mut dyn Write,
) -> Result<RunLock, StandError> {
    let started = Instant::now();
    let mut said = false;
    loop {
        if let Some(lock) = store.try_lock()? {
            return Ok(lock);
        }
        if cancel.is_cancelled() {
            return Err(StandError::Cancelled);
        }
        if !said {
            // A stderr that fails has nowhere left to report it.
            let _ = writeln!(err, "waiting for the stand");
            said = true;
        }
        let waited = started.elapsed();
        if waited >= limit {
            return Err(StandError::LockWait(limit));
        }
        thread::sleep(POLL.min(limit - waited));
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use store::StorePaths;

    use super::*;

    fn store(dir: &Utf8Path) -> Store {
        Store::open(StorePaths {
            root: dir.join("root"),
            config: dir.join("stand.toml"),
        })
        .unwrap()
    }

    #[test]
    fn the_limit_is_the_span_of_the_stand_or_the_default_span() {
        let dir = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(dir.path()).unwrap();
        std::fs::create_dir_all(dir.join("recipe")).unwrap();
        let file = dir.join("recipe/compose.yaml");
        std::fs::write(
            &file,
            "services:\n  w:\n    image: alpine\n    deploy: { resources: { limits: { cpus: \"1\", memory: 64m } } }\n\
             x-stand: { workload: w, repeat: 2, timeout: 1m }\n",
        )
        .unwrap();
        let stand = spec::load(&file, &spec::Policy::new(None).unwrap()).unwrap();
        assert_eq!(limit(Some(&stand)), run::span(&stand));
        assert_ne!(run::span(&stand), run::DEFAULT_SPAN);
        assert_eq!(limit(None), run::DEFAULT_SPAN);
    }

    #[test]
    fn a_held_lock_with_no_time_to_wait_says_waiting_once_and_gives_up() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(Utf8Path::from_path(dir.path()).unwrap());
        let _held = store.try_lock().unwrap().expect("the first lock");
        let mut err = Vec::new();
        let waited = wait(&store, Duration::ZERO, &Cancel::new(), &mut err);
        assert!(
            matches!(waited, Err(StandError::LockWait(limit)) if limit.is_zero()),
            "{:?}",
            waited.err()
        );
        assert_eq!(String::from_utf8(err).unwrap(), "waiting for the stand\n");
    }

    #[test]
    fn ctrl_c_ends_the_wait() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(Utf8Path::from_path(dir.path()).unwrap());
        let _held = store.try_lock().unwrap().expect("the first lock");
        let cancel = Cancel::new();
        cancel.cancel();
        let waited = wait(&store, Duration::from_secs(3600), &cancel, &mut Vec::new());
        assert!(
            matches!(waited, Err(StandError::Cancelled)),
            "{:?}",
            waited.err()
        );
    }
}
