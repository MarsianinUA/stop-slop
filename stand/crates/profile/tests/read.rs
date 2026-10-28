use std::fmt::Write as _;
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use profile::{HotLine, ProfileError, Samples, read};
use tempfile::TempDir;

/// A copy of `tests/fixtures/<case>` in a folder whose path has a space, with `{root}` and
/// `{url}` in its files replaced by the path and the file URL of that folder. The test reads it
/// both as the profile folder and as the repo root.
///
/// The fixtures are written by hand, except `maps/dist/orders.js` and its map: in
/// `fixtures/maps`, `tsc --sourceMap --target es2022 --module commonjs --experimentalDecorators
/// --outDir dist src/orders.ts` with TypeScript 5.9. Its `orders.js.map` is not kept as a file:
/// `100.0.maps.json` holds it, as the preload writes it, and the inline maps copy its `mappings`.
/// The frames use V8's positions in that output, from 0: `__decorate` at 1:55, `total` at 24:14,
/// `report` at 31:15, `fib` at 34:12; every sample lasts 1 ms unless the test says otherwise.
fn fixture(case: &str) -> (TempDir, Utf8PathBuf) {
    let temp = tempfile::tempdir().expect("create a temp folder");
    let root = Utf8PathBuf::try_from(temp.path().join("a repo")).expect("a UTF-8 temp folder");
    let from = Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(case);
    let slashed = root.as_str().replace('\\', "/");
    copy(&from, &root, &slashed, &file_url(&slashed));
    (temp, root)
}

fn copy(from: &Utf8Path, to: &Utf8Path, root: &str, url: &str) {
    fs::create_dir_all(to).unwrap();
    for entry in from.read_dir_utf8().unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(entry.path(), &target, root, url);
        } else {
            let text = fs::read_to_string(entry.path()).unwrap();
            fs::write(target, text.replace("{root}", root).replace("{url}", url)).unwrap();
        }
    }
}

/// `file:///C:/a%20repo` on Windows, `file:///tmp/a%20repo` elsewhere.
fn file_url(slashed: &str) -> String {
    let mut url = String::from(if slashed.starts_with('/') {
        "file://"
    } else {
        "file:///"
    });
    for byte in slashed.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte) {
            url.push(char::from(byte));
        } else {
            write!(url, "%{byte:02X}").unwrap();
        }
    }
    url
}

fn relative(file: &Utf8Path, root: &Utf8Path) -> String {
    let rest = file
        .strip_prefix(root)
        .unwrap_or_else(|_| panic!("{file} is not under {root}"));
    rest.as_str().replace('\\', "/")
}

/// File, line, column, self and total time.
fn functions(samples: &Samples, root: &Utf8Path) -> Vec<(String, u32, u32, f64, f64)> {
    samples
        .functions
        .iter()
        .map(|f| {
            (
                relative(&f.file, root),
                f.line,
                f.column,
                f.self_ms,
                f.total_ms,
            )
        })
        .collect()
}

fn modules(samples: &Samples, root: &Utf8Path) -> Vec<(String, f64)> {
    samples
        .modules
        .iter()
        .map(|module| (relative(&module.file, root), module.self_ms))
        .collect()
}

fn totals(samples: &Samples) -> (f64, f64, f64, u32, u32) {
    (
        samples.total_ms,
        samples.local_ms,
        samples.unmapped_ms,
        samples.processes,
        samples.lost_processes,
    )
}

#[test]
fn functions_map_through_an_inline_map_and_through_a_map_file() {
    let (_temp, root) = fixture("maps");
    let samples = read(&root, &root).unwrap();
    // `total` runs in `{url}/src/orders.ts`, as ts-jest compiles it, with the inline map of
    // `100.0.maps.json`; `fib` runs in `dist/orders.js`, whose map comes from
    // `dist/orders.js.map`, a file the folder does not have: its relative source starts from
    // that path. Chunk 1 has no maps of its own and adds 2 ms of `total` to the 4 ms of chunk 0.
    assert_eq!(
        functions(&samples, &root),
        [
            ("src/orders.ts".to_owned(), 14, 22, 6.0, 6.0),
            ("src/orders.ts".to_owned(), 26, 20, 2.0, 2.0),
        ]
    );
    assert_eq!(totals(&samples), (11.0, 11.0, 0.0, 1, 0));
}

#[test]
fn top_level_code_and_compiler_helpers_go_to_the_module_of_their_file() {
    let (_temp, root) = fixture("maps");
    let samples = read(&root, &root).unwrap();
    // 1 ms at 0:0 of `{url}/src/orders.ts`, and 1 ms in `__decorate` of `dist/orders.js`, which
    // its map has no original for. `dist/esm.js` has a mapping at 0:0, as ESM output has.
    assert_eq!(
        modules(&samples, &root),
        [
            ("src/esm.ts".to_owned(), 1.0),
            ("src/orders.ts".to_owned(), 2.0),
        ]
    );
}

#[test]
fn lines_map_to_the_source_hottest_first() {
    let (_temp, root) = fixture("maps");
    let samples = read(&root, &root).unwrap();
    let lines: Vec<&[HotLine]> = samples.functions.iter().map(|f| &f.lines[..]).collect();
    // Generated lines 28 and 27 of `total` are lines 17 and 16 of `orders.ts`, line 36 of
    // `fib` is line 27: the first mapping of each line, not the one before its indentation.
    assert_eq!(
        lines,
        [
            &[HotLine { line: 17, ms: 5.0 }, HotLine { line: 16, ms: 1.0 }][..],
            &[HotLine { line: 27, ms: 2.0 }][..],
        ]
    );
}

#[test]
fn recursion_counts_once_per_sample_and_processes_add_up() {
    let (_temp, root) = fixture("processes");
    let samples = read(&root, &root).unwrap();
    // In 200, `fib` is three deep under 4 of its samples, and under 1 of `node_modules`; in
    // 300 it has 2 more. `report` calls `total`.
    assert_eq!(
        functions(&samples, &root),
        [
            ("src/orders.ts".to_owned(), 14, 22, 3.0, 3.0),
            ("src/orders.ts".to_owned(), 22, 23, 1.0, 4.0),
            ("src/orders.ts".to_owned(), 26, 20, 6.0, 7.0),
        ]
    );
}

#[test]
fn a_map_serves_only_its_own_process() {
    let (_temp, root) = fixture("processes");
    let samples = read(&root, &root).unwrap();
    // 400 runs the same script as 200 and 300 but has no maps: its 3 ms stay unmapped.
    assert_eq!(totals(&samples), (14.0, 13.0, 3.0, 3, 0));
}

#[test]
fn ts_without_a_map_is_unmapped_and_js_of_the_repo_is_its_own_source() {
    let (_temp, root) = fixture("kinds");
    let samples = read(&root, &root).unwrap();
    // 1 ms each in `src/legacy.js` (in `parse` and at its top level), in `src/orders.ts`
    // without a map (its URL has a query, as a re-imported ES module has), in `node_modules`,
    // in `node:internal/timers`; 4 ms in `(program)`, the last sample, which lasts until the
    // end of the profile.
    assert_eq!(
        functions(&samples, &root),
        [("src/legacy.js".to_owned(), 3, 21, 1.0, 1.0)]
    );
    assert_eq!(samples.functions[0].lines, [HotLine { line: 4, ms: 1.0 }]);
    assert_eq!(
        modules(&samples, &root),
        [("src/legacy.js".to_owned(), 1.0)]
    );
    assert_eq!(totals(&samples), (9.0, 3.0, 1.0, 1, 0));
}

#[test]
fn windows_ignores_the_case_of_file_names() {
    let (_temp, root) = fixture("case");
    let samples = read(&root, &root).unwrap();
    // 1 ms of `parse` in `src/legacy.js` and 1 ms in `SRC/Legacy.js`.
    let expected = if cfg!(windows) {
        vec![("src/legacy.js".to_owned(), 3, 21, 2.0, 2.0)]
    } else {
        vec![
            ("SRC/Legacy.js".to_owned(), 3, 21, 1.0, 1.0),
            ("src/legacy.js".to_owned(), 3, 21, 1.0, 1.0),
        ]
    };
    assert_eq!(functions(&samples, &root), expected);
}

#[test]
fn a_process_with_a_start_mark_and_no_chunk_is_lost() {
    let (_temp, root) = fixture("lost");
    let samples = read(&root, &root).unwrap();
    // 700 left only a `.tmp` of its first chunk; `preload.cjs` is no profile either.
    assert_eq!(totals(&samples), (1.0, 0.0, 0.0, 2, 1));
}

#[test]
fn no_chunk_is_no_profile_and_broken_json_names_its_file() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8Path::from_path(temp.path()).unwrap();
    assert!(matches!(read(dir, dir), Err(ProfileError::NoProfile)));

    fs::write(dir.join("1.start.json"), "{}").unwrap();
    assert!(matches!(read(dir, dir), Err(ProfileError::NoProfile)));

    fs::write(dir.join("1.0.cpuprofile"), r#"{"nodes": ["#).unwrap();
    match read(dir, dir) {
        Err(ProfileError::BadProfile { path, message }) => {
            assert_eq!(path, dir.join("1.0.cpuprofile"));
            assert!(message.contains("EOF"), "{message}");
        }
        other => panic!("expected a bad profile, got {other:?}"),
    }
}
