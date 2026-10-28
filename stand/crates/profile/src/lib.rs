use std::io;

use camino::{Utf8Path, Utf8PathBuf};

mod env;
mod maps;
mod paths;
mod read;

pub use env::node_env;
pub use paths::PathMap;
pub use read::read;

/// The file the caller writes as `<preload_dir>/preload.cjs` of `node_env`.
pub const PRELOAD: &str = include_str!("preload.cjs");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileMode {
    Cpu,
    Coverage,
}

/// `NODE_OPTIONS`, `STOP_SLOP_PROFILE_DIR`, `STOP_SLOP_PROFILE_MODE`.
#[derive(Debug, PartialEq, Eq)]
pub struct NodeEnv {
    pub node_options: String,
    pub profile_dir: String,
    pub profile_mode: &'static str,
}

#[derive(Debug)]
pub struct Samples {
    pub total_ms: f64,
    /// Samples whose top frame is a local file (`PathMap`).
    pub local_ms: f64,
    /// The part of `local_ms` in `.ts`, `.tsx`, `.mts` and `.cts` files that found no source map.
    pub unmapped_ms: f64,
    /// Node processes and worker threads the preload started in.
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
    /// The original source after the source map, from the repo root and `/`-separated.
    pub file: Utf8PathBuf,
    /// From 1: where V8 puts the function start, the `(` of its parameters.
    pub line: u32,
    pub column: u32,
    /// V8's function name; empty for an anonymous one.
    pub name: String,
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
    /// From the repo root and `/`-separated.
    pub file: Utf8PathBuf,
    pub self_ms: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("no Node process wrote a profile: the service must run Node 20 or later")]
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

    fn bad(path: &Utf8Path, message: String) -> ProfileError {
        ProfileError::BadProfile {
            path: path.to_owned(),
            message,
        }
    }
}
