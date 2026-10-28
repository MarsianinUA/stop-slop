use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::Command;

use camino::Utf8Path;

use crate::{Exited, ProfileError};

const PRELOAD: &str = include_str!("preload.cjs");

/// Runs `command` and waits for it, with its input and output untouched. Every Node process it
/// starts writes its CPU profile into `dir`, an existing empty folder that `read` reads
/// afterwards.
pub fn run(command: &[OsString], dir: &Utf8Path) -> Result<Exited, ProfileError> {
    let (program, args) = command.split_first().ok_or_else(|| ProfileError::Spawn {
        program: String::new(),
        source: io::Error::new(io::ErrorKind::InvalidInput, "the command is empty"),
    })?;
    // Node resolves `--require` against the working folder of each process, which the command
    // may change.
    let dir = camino::absolute_utf8(dir).map_err(ProfileError::io(dir))?;
    let preload = dir.join("preload.cjs");
    fs::write(&preload, PRELOAD).map_err(ProfileError::io(&preload))?;
    let status = Command::new(resolve(program))
        .args(args)
        .env("NODE_OPTIONS", node_options(&preload))
        .env("STOP_SLOP_PROFILE_DIR", &dir)
        .status()
        .map_err(|source| ProfileError::Spawn {
            program: program.to_string_lossy().into_owned(),
            source,
        })?;
    Ok(status.code().map_or(Exited::Signal, Exited::Code))
}

/// The preload goes first, so the profile also covers what the user's own `--require` runs.
/// Node splits `NODE_OPTIONS` at spaces outside `"`, and inside `"` a `\` escapes the next
/// character: `C:\Users\a b\preload.cjs` is written `"C:\\Users\\a b\\preload.cjs"`.
fn node_options(preload: &Utf8Path) -> OsString {
    let mut options = String::from("--require \"");
    for ch in preload.as_str().chars() {
        if matches!(ch, '\\' | '"') {
            options.push('\\');
        }
        options.push(ch);
    }
    options.push('"');
    let mut options = OsString::from(options);
    if let Some(previous) = env::var_os("NODE_OPTIONS") {
        options.push(" ");
        options.push(previous);
    }
    options
}

#[cfg(not(windows))]
fn resolve(program: &OsStr) -> PathBuf {
    PathBuf::from(program)
}

/// `Command` looks only for `<name>.exe`, while `npm`, `pnpm` and `node_modules/.bin/jest` are
/// `.cmd` files. `Command` runs a `.cmd` or `.bat` path through `cmd.exe` and escapes its
/// arguments for it. The other extensions of `PATHEXT`, such as `.js`, need a program that
/// `Command` does not start, so the search tries only these four, in the default order of
/// `PATHEXT`. Not found, the name goes to `Command` as given, which fails to start it.
#[cfg(windows)]
fn resolve(program: &OsStr) -> PathBuf {
    use std::path::Path;

    const EXTENSIONS: [&str; 4] = [".com", ".exe", ".bat", ".cmd"];
    let name = Path::new(program);
    let bases: Vec<PathBuf> = if name.parent().is_some_and(|dir| !dir.as_os_str().is_empty()) {
        vec![name.to_owned()]
    } else {
        env::var_os("PATH").map_or_else(Vec::new, |path| {
            env::split_paths(&path).map(|dir| dir.join(name)).collect()
        })
    };
    for base in bases {
        if base.extension().is_some() && base.is_file() {
            return base;
        }
        for extension in EXTENSIONS {
            let mut candidate = base.clone().into_os_string();
            candidate.push(extension);
            let candidate = PathBuf::from(candidate);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    name.to_owned()
}
