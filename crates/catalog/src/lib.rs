mod error;
mod fixture;
mod render;
mod scan;
mod schema;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub use error::{FolderError, Problem};
pub use fixture::{Expected, Fixture, read_fixture};

#[derive(Debug)]
pub struct Generated {
    pub code: String,
    /// `rules_dir` and its group folders, for `cargo::rerun-if-changed`.
    pub watch: Vec<PathBuf>,
}

/// Checks every folder under `rules_dir` and returns the text of `registry.rs`, or every
/// problem found. `rules_dir` must be absolute: it goes into `#[path]` of the generated file,
/// which does not live next to the rules.
pub fn generate(rules_dir: &Path) -> Result<Generated, Vec<FolderError>> {
    let catalog = scan::scan(rules_dir)?;
    let groups = catalog
        .groups
        .iter()
        .map(|group| rules_dir.join(&group.name));
    Ok(Generated {
        code: render::render(&catalog),
        watch: std::iter::once(rules_dir.to_path_buf())
            .chain(groups)
            .collect(),
    })
}

/// Leaves an unchanged file untouched, so its modification time does not trigger a rebuild.
pub fn write_if_changed(path: &Path, code: &str) -> io::Result<()> {
    match fs::read(path) {
        Ok(old) if old == code.as_bytes() => Ok(()),
        _ => fs::write(path, code),
    }
}
