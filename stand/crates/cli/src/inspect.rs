//! `inspect <run> <side> <service> -- <argv>` (plan, row 64): a look at the end state of the kept
//! run, in a throwaway copy of its snapshot. No verdict: the code of `argv` is the first line, never
//! the exit code. The budget is not counted again: one VM within the limits the run had.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::time::SystemTime;

use run::{InspectError, Inspected, Side};
use sandbox::{ExecOutcome, Sandbox};

use crate::error::StandError;
use crate::{Env, Exit, Loaded, slashed};

/// Lines of stdout printed; the file holds the rest.
const LINES: usize = 40;

pub(crate) fn run(
    run_id: &str,
    side: Side,
    service: &str,
    argv: &[String],
    sandbox: &impl Sandbox,
    env: &mut Env<'_>,
) -> Result<Exit, StandError> {
    let loaded = Loaded::read(env)?;
    let (_lock, session) = loaded.session(env, SystemTime::now())?;
    let kept = match session.kept()? {
        Some(kept) if kept.id == run_id => kept,
        other => {
            let held = other.map_or("which keeps none".to_owned(), |kept| {
                format!("which keeps {}", kept.id)
            });
            let refused = format!("{run_id} is not the kept run of this work tree, {held}");
            return Err(InspectError::NotKept(refused).into());
        }
    };
    let inspected = run::inspect(sandbox, &kept.dir, side, service, argv, env.cancel)?;
    print(&inspected, env.out)?;
    Ok(Exit::Passed)
}

fn print(inspected: &Inspected, out: &mut dyn Write) -> Result<(), StandError> {
    let path = &inspected.out;
    let stdout = |source| StandError::Io {
        path: "stdout".into(),
        source,
    };
    let read = |source| StandError::Io {
        path: path.clone(),
        source,
    };
    match inspected.outcome {
        ExecOutcome::Exited(code) => writeln!(out, "exit {code}"),
        ExecOutcome::TimedOut => writeln!(out, "timeout"),
    }
    .map_err(stdout)?;
    // `<run>/inspect/` is a writable bind of the guest, so `argv` may have left a link or a folder
    // there; a link is never followed. The VM is gone, so what is checked here stays.
    let regular = fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file());
    if !regular {
        let said = format!("the output {} is not a regular file", slashed(path));
        return writeln!(out, "{said}; nothing of it is printed").map_err(stdout);
    }
    let file = File::open(path).map_err(read)?;
    for line in BufReader::new(file).split(b'\n').take(LINES) {
        let line = line.map_err(read)?;
        let line = line.strip_suffix(b"\r").unwrap_or(&line);
        writeln!(out, "{}", String::from_utf8_lossy(line)).map_err(stdout)?;
    }
    writeln!(out, "full output {}", slashed(path)).map_err(stdout)
}
