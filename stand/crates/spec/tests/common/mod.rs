use camino::{Utf8Path, Utf8PathBuf};
use spec::{Policy, SpecError, Stand, load};

/// `tests/fixtures/<case>/compose.yaml`. Every refusal has its own folder: a small file that is
/// valid but for the one thing the case is named after.
pub fn fixture(case: &str) -> Utf8PathBuf {
    Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(case)
        .join("compose.yaml")
}

/// With the default policy, `max_tolerance` 25.
pub fn load_case(case: &str) -> Result<Stand, SpecError> {
    load(&fixture(case), &Policy::new(None).unwrap())
}

/// Each case is refused at its key and line, with a message that has the words.
pub fn assert_refused(cases: &[(&str, &str, u32, &str)]) {
    for &(case, key, line, words) in cases {
        let Err(SpecError::Invalid {
            file,
            line: refused_line,
            key: refused_key,
            message,
        }) = load_case(case)
        else {
            panic!("{case} is not refused as invalid");
        };
        assert!(
            file.ends_with(format!("{case}/compose.yaml")),
            "{case}: {file}"
        );
        assert_eq!(
            (refused_key.as_str(), refused_line),
            (key, line),
            "{case}: {message}"
        );
        assert!(message.contains(words), "{case}: {message}");
    }
}
