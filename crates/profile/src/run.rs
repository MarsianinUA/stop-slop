use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::Command;

use camino::Utf8Path;

use crate::{Exited, ProfileError};

const PRELOAD: &str = include_str!("preload.cjs");

pub(crate) fn run(command: &[OsString], dir: &Utf8Path) -> Result<Exited, ProfileError> {
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
/// arguments for it. Not found, the name goes to `Command` as given, which fails to start it.
#[cfg(windows)]
fn resolve(program: &OsStr) -> PathBuf {
    use std::path::Path;

    let name = Path::new(program);
    let dirs: Vec<PathBuf> = if name.parent().is_some_and(|dir| !dir.as_os_str().is_empty()) {
        vec![PathBuf::new()]
    } else {
        env::var_os("PATH").map_or_else(Vec::new, |path| env::split_paths(&path).collect())
    };
    let extensions = env::var_os("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
    let extensions = extensions.to_string_lossy();
    for dir in dirs {
        let base = dir.join(name);
        if base.extension().is_some() && base.is_file() {
            return base;
        }
        for extension in extensions
            .split(';')
            .filter(|extension| !extension.is_empty())
        {
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
