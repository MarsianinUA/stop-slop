use std::ffi::OsString;
use std::io;

use camino::{Utf8Path, Utf8PathBuf};

mod maps;
mod read;
mod run;

/// Runs `command` and waits for it, with its input and output untouched. Every Node process it
/// starts writes its CPU profile into `dir`, an existing empty folder that `read` reads
/// afterwards.
pub fn run(command: &[OsString], dir: &Utf8Path) -> Result<Exited, ProfileError> {
    run::run(command, dir)
}

/// Sums the profiles `run` left in `dir` by function of the original sources.
pub fn read(dir: &Utf8Path, root: &Utf8Path) -> Result<Samples, ProfileError> {
    read::read(dir, root)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exited {
    Code(i32),
    /// Only on Unix: on Windows a killed process has an exit code too.
    Signal,
}

#[derive(Debug)]
pub struct Samples {
    pub total_ms: f64,
    /// Samples whose top frame is a file under `root` outside `node_modules`.
    pub local_ms: f64,
    /// The part of `local_ms` in `.ts`, `.tsx`, `.mts` and `.cts` files that found no source map.
    pub unmapped_ms: f64,
    /// Node processes the preload started in.
    pub processes: u32,
    /// Of `processes`, those killed before they wrote a single chunk.
    pub lost_processes: u32,
    /// Local functions, by file, line and column.
    pub functions: Vec<Sampled>,
    /// By file.
    pub modules: Vec<ModuleTime>,
}

#[derive(Debug)]
pub struct Sampled {
    /// Absolute path of the original source, after the source map.
    pub file: Utf8PathBuf,
    /// From 1: where V8 puts the function start, the `(` of its parameters.
    pub line: u32,
    pub column: u32,
    pub self_ms: f64,
    /// A recursive function counts once per sample.
    pub total_ms: f64,
    /// Hottest first.
    pub lines: Vec<HotLine>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HotLine {
    pub line: u32,
    pub ms: f64,
}

/// Top-level code of a file: frames of the generated code with no mapping, such as the module
/// body at 1:1 and the helpers `tsc` puts before the first mapping (`__decorate`, `__exportStar`).
#[derive(Debug)]
pub struct ModuleTime {
    pub file: Utf8PathBuf,
    pub self_ms: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("cannot start `{program}`: {source}")]
    Spawn { program: String, source: io::Error },
    #[error("no Node process wrote a profile: the command must start Node 20 or later")]
    NoProfile,
    #[error("{path}: {message}")]
    BadProfile { path: Utf8PathBuf, message: String },
    #[error("{path}: {source}")]
    Io {
        path: Utf8PathBuf,
        source: io::Error,
    },
}

impl ProfileError {
    fn io(path: &Utf8Path) -> impl Fn(io::Error) -> ProfileError {
        move |source| ProfileError::Io {
            path: path.to_owned(),
            source,
        }
    }
}
