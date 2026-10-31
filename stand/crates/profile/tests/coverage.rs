use std::fs;

use camino::Utf8Path;
use profile::{Coverage, PathMap, ProfileError, read_coverage};

/// File, line, column, name and calls.
fn called(coverage: &Coverage) -> Vec<(&str, u32, u32, &str, u64)> {
    coverage
        .called
        .iter()
        .map(|f| (f.file.as_str(), f.line, f.column, f.name.as_str(), f.calls))
        .collect()
}

/// `tests/fixtures/coverage`, from a guest whose tree is `/w/app`. Process 100 is what Node
/// 24.22.1 wrote running the folder's `main.js` beside the `dist/` of `fixtures/maps` (with its
/// map back in a file), its URLs and paths then moved under `/w/app`: chunk 0 at the preload's
/// 2 s timer, chunk 1 at the exit, after the calls `main.js` makes at 2.3 s. Process 200 is
/// written by hand.
#[test]
fn calls_of_the_functions_of_the_tree_map_to_their_starts_in_the_sources() {
    let dir = Utf8Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/coverage");
    let coverage = read_coverage(&dir, &PathMap::new("/w/app")).unwrap();
    // `total` has 2 calls in chunk 0 and 1 in chunk 1, `fib` 15 in chunk 1. V8 starts the
    // declarations of `dist/orders.js` at `function`, which the map gives as the `export` of
    // `orders.ts`; `logged` is not exported. `main.js` has no map: it is its own source, and the
    // arrow function V8 starts at its parameters is anonymous. `add` starts at 0:0 like the
    // top-level code of its script, which does not count. Not there: the uncalled `list`,
    // `__decorate`, which the map has no original for, `/w/app-tools/run.js` outside the tree,
    // and `src/legacy.ts` without a map.
    assert_eq!(
        called(&coverage),
        [
            ("main.js", 5, 12, "", 1),
            ("src/add.js", 1, 1, "add", 3),
            ("src/orders.ts", 3, 1, "logged", 1),
            ("src/orders.ts", 14, 1, "total", 3),
            ("src/orders.ts", 22, 1, "report", 1),
            ("src/orders.ts", 26, 1, "fib", 15),
        ]
    );
}

#[test]
fn no_coverage_chunk_is_no_profile_and_broken_json_names_its_file() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8Path::from_path(temp.path()).unwrap();
    let paths = PathMap::new("/w/app");
    assert!(matches!(
        read_coverage(dir, &paths),
        Err(ProfileError::NoProfile)
    ));

    // A CPU profile is not coverage.
    fs::write(dir.join("1.start.json"), "{}").unwrap();
    fs::write(dir.join("1.0.cpuprofile"), "{}").unwrap();
    assert!(matches!(
        read_coverage(dir, &paths),
        Err(ProfileError::NoProfile)
    ));

    fs::write(dir.join("1.0.coverage.json"), "[{").unwrap();
    match read_coverage(dir, &paths) {
        Err(ProfileError::BadProfile { path, message }) => {
            assert_eq!(path, dir.join("1.0.coverage.json"));
            assert!(message.contains("EOF"), "{message}");
        }
        other => panic!("expected a bad profile, got {other:?}"),
    }
}
