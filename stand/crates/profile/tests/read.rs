use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use profile::{HotLine, PathMap, ProfileError, Samples, read};

/// The profile folder `tests/fixtures/<case>`, as `copy_out` brings it from a guest whose tree is
/// `/w/app`: its URLs and paths are the guest's, and none of them is on the host.
///
/// The fixtures are written by hand, except `maps/dist/orders.js` and its map: in
/// `fixtures/maps`, `tsc --sourceMap --target es2022 --module commonjs --experimentalDecorators
/// --outDir dist src/orders.ts` with TypeScript 5.9. Its `orders.js.map` is not kept as a file:
/// `100.0.maps.json` holds it, as the preload writes it, and the inline maps copy its `mappings`.
/// The frames use V8's positions in that output, from 0: `__decorate` at 1:55, `total` at 24:14,
/// `report` at 31:15, `fib` at 34:12; every sample lasts 1 ms unless the test says otherwise.
fn fixture(case: &str) -> Utf8PathBuf {
    Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(case)
}

fn read_guest(case: &str) -> Samples {
    read(&fixture(case), &PathMap::new("/w/app")).unwrap()
}

/// File, line, column, self and total time.
fn functions(samples: &Samples) -> Vec<(&str, u32, u32, f64, f64)> {
    samples
        .functions
        .iter()
        .map(|f| (f.file.as_str(), f.line, f.column, f.self_ms, f.total_ms))
        .collect()
}

fn modules(samples: &Samples) -> Vec<(&str, f64)> {
    samples
        .modules
        .iter()
        .map(|module| (module.file.as_str(), module.self_ms))
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
    let samples = read_guest("maps");
    // `total` runs in `/w/app/src/orders.ts`, as ts-jest compiles it, with the inline map of
    // `100.0.maps.json`; `fib` runs in `dist/orders.js`, whose map came from the guest's
    // `dist/orders.js.map`: only `maps.json` has it, and its relative source starts from that
    // path. Chunk 1 has no maps of its own and adds 2 ms of `total` to the 4 ms of chunk 0.
    assert_eq!(
        functions(&samples),
        [
            ("src/orders.ts", 14, 22, 6.0, 6.0),
            ("src/orders.ts", 26, 20, 2.0, 2.0),
        ]
    );
    assert_eq!(totals(&samples), (11.0, 11.0, 0.0, 1, 0));
}

#[test]
fn top_level_code_and_compiler_helpers_go_to_the_module_of_their_file() {
    let samples = read_guest("maps");
    // 1 ms at 0:0 of `/w/app/src/orders.ts`, and 1 ms in `__decorate` of `dist/orders.js`, which
    // its map has no original for. `dist/esm.js` has a mapping at 0:0, as ESM output has.
    assert_eq!(
        modules(&samples),
        [("src/esm.ts", 1.0), ("src/orders.ts", 2.0)]
    );
}

#[test]
fn lines_map_to_the_source_hottest_first() {
    let samples = read_guest("maps");
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
    let samples = read_guest("processes");
    // In 200, `fib` is three deep under 4 of its samples, and under 1 of `node_modules`; in
    // 300 it has 2 more. `report` calls `total`.
    assert_eq!(
        functions(&samples),
        [
            ("src/orders.ts", 14, 22, 3.0, 3.0),
            ("src/orders.ts", 22, 23, 1.0, 4.0),
            ("src/orders.ts", 26, 20, 6.0, 7.0),
        ]
    );
}

#[test]
fn a_map_serves_only_its_own_process() {
    let samples = read_guest("processes");
    // 400 runs the same script as 200 and 300 but has no maps: its 3 ms stay unmapped.
    assert_eq!(totals(&samples), (14.0, 13.0, 3.0, 3, 0));
}

#[test]
fn ts_without_a_map_is_unmapped_and_only_files_of_the_tree_are_local() {
    let samples = read_guest("kinds");
    // 1 ms each in `src/legacy.js` (in `parse` and at its top level), in `src/start up.js`, in
    // `src/orders.ts` without a map (its URL has a query, as a re-imported ES module has), in
    // `node_modules`, in `node:internal/timers`, in `/w/app-tools/run.js`, which only starts
    // like the tree; 4 ms in `(program)`, the last sample, which lasts until the end of the profile.
    assert_eq!(
        functions(&samples),
        [
            ("src/legacy.js", 3, 21, 1.0, 1.0),
            ("src/start up.js", 2, 11, 1.0, 1.0),
        ]
    );
    assert_eq!(samples.functions[0].lines, [HotLine { line: 4, ms: 1.0 }]);
    assert_eq!(modules(&samples), [("src/legacy.js", 1.0)]);
    assert_eq!(totals(&samples), (11.0, 4.0, 1.0, 1, 0));
}

#[test]
fn sources_resolve_after_the_source_root_and_a_drive_letter_is_no_guest_path() {
    let samples = read_guest("sources");
    // `dist/a.js` maps to `a.ts` under its `sourceRoot` `../src`, from the folder of its map. The
    // map of `dist/c.js`, built on Windows, names only `C:/src/c.ts`, which is no file of the
    // guest: `c.js` is its own source, as with a `webpack://` source.
    assert_eq!(
        functions(&samples),
        [("dist/c.js", 1, 11, 1.0, 1.0), ("src/a.ts", 1, 1, 1.0, 1.0)]
    );
}

#[test]
fn a_function_has_v8s_name_and_an_anonymous_one_an_empty_name() {
    let names = |case| {
        read_guest(case)
            .functions
            .into_iter()
            .map(|f| f.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(names("processes"), ["total", "report", "fib"]);
    // `src/start up.js` runs an anonymous function.
    assert_eq!(names("kinds"), ["parse", ""]);
}

#[test]
fn the_case_of_guest_file_names_counts_on_every_host() {
    let samples = read_guest("case");
    // 1 ms of `parse` in `src/legacy.js` and 1 ms in `SRC/Legacy.js`: two files on Linux.
    assert_eq!(
        functions(&samples),
        [
            ("SRC/Legacy.js", 3, 21, 1.0, 1.0),
            ("src/legacy.js", 3, 21, 1.0, 1.0),
        ]
    );
}

#[test]
fn a_process_with_a_start_mark_and_no_chunk_is_lost() {
    let samples = read_guest("lost");
    // 700 left only a `.tmp` of its first chunk; `preload.cjs` is no profile either.
    assert_eq!(totals(&samples), (1.0, 0.0, 0.0, 2, 1));
}

/// The setup cache and the report skip such a name too (batch review 6).
#[test]
fn a_name_that_is_not_utf8_is_skipped() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8Path::from_path(temp.path()).unwrap();
    for entry in fixture("lost").read_dir_utf8().unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }
    #[cfg(unix)]
    let odd = {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::OsStr::from_bytes(b"\xff.0.cpuprofile").to_owned()
    };
    #[cfg(windows)]
    let odd = {
        use std::os::windows::ffi::OsStringExt;
        let mut wide = vec![0xD800];
        wide.extend(".0.cpuprofile".encode_utf16());
        std::ffi::OsString::from_wide(&wide)
    };
    // APFS refuses such a name: macOS has nothing to skip.
    if fs::write(dir.as_std_path().join(odd), "not a profile").is_err() {
        return;
    }
    let samples = read(dir, &PathMap::new("/w/app")).unwrap();
    assert_eq!(totals(&samples), totals(&read_guest("lost")));
}

#[test]
fn no_chunk_is_no_profile_and_broken_json_names_its_file() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8Path::from_path(temp.path()).unwrap();
    let paths = PathMap::new("/w/app");
    assert!(matches!(read(dir, &paths), Err(ProfileError::NoProfile)));

    fs::write(dir.join("1.start.json"), "{}").unwrap();
    assert!(matches!(read(dir, &paths), Err(ProfileError::NoProfile)));

    fs::write(dir.join("1.0.cpuprofile"), r#"{"nodes": ["#).unwrap();
    match read(dir, &paths) {
        Err(ProfileError::BadProfile { path, message }) => {
            assert_eq!(path, dir.join("1.0.cpuprofile"));
            assert!(message.contains("EOF"), "{message}");
        }
        other => panic!("expected a bad profile, got {other:?}"),
    }
}
