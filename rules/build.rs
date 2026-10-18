#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

use std::env;
use std::path::PathBuf;

fn main() {
    let rules_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let generated = catalog::generate(&rules_dir).unwrap_or_else(|errors| {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("broken rule folders:\n{}", lines.join("\n"));
    });
    for path in &generated.watch {
        println!("cargo::rerun-if-changed={}", path.display());
    }
    let registry = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("registry.rs");
    catalog::write_if_changed(&registry, &generated.code).unwrap();
}
