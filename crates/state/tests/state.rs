use std::fs;
use std::path::{Path, PathBuf};

use state::{Config, IgnoreEntry, State, StateError};
use tempfile::TempDir;

fn folder(temp: &TempDir) -> PathBuf {
    temp.path().join("info").join("stop-slop")
}

fn entry(file: &str, fingerprint: &str, reason: &str) -> IgnoreEntry {
    let (rule, _) = fingerprint.split_once(':').expect("rule:hash");
    IgnoreEntry {
        fingerprint: fingerprint.to_owned(),
        rule: rule.to_owned(),
        file: file.into(),
        reason: reason.to_owned(),
        added: "2026-10-15".to_owned(),
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|item| item.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn parse_error<T: std::fmt::Debug>(result: Result<T, StateError>) -> (PathBuf, u32, String) {
    match result {
        Err(StateError::Parse {
            path,
            line,
            message,
        }) => (path, line, message),
        other => panic!("expected a parse error, got {other:?}"),
    }
}

fn io_error_path<T: std::fmt::Debug>(result: Result<T, StateError>) -> PathBuf {
    match result {
        Err(StateError::Io { path, .. }) => path,
        other => panic!("expected an I/O error, got {other:?}"),
    }
}

fn write(temp: &TempDir, name: &str, text: &str) {
    fs::create_dir_all(folder(temp)).unwrap();
    fs::write(folder(temp).join(name), text).unwrap();
}

#[test]
fn disable_removes_only_the_marker() {
    let temp = TempDir::new().unwrap();
    let state = State::open(temp.path());
    assert!(!state.is_enabled());

    state.enable().unwrap();
    assert!(folder(&temp).join("enabled").is_file());
    assert!(state.is_enabled());
    state.enable().unwrap();
    assert!(state.is_enabled());

    state
        .add_ignore(vec![entry("src/a.ts", "x:111111", "r")])
        .unwrap();
    fs::write(folder(&temp).join("config.toml"), "disabled = [\"x\"]\n").unwrap();
    let ignore = fs::read(folder(&temp).join("ignore.json")).unwrap();
    let config = fs::read(folder(&temp).join("config.toml")).unwrap();

    state.disable().unwrap();
    assert!(!state.is_enabled());
    assert!(!folder(&temp).join("enabled").exists());
    assert_eq!(fs::read(folder(&temp).join("ignore.json")).unwrap(), ignore);
    assert_eq!(fs::read(folder(&temp).join("config.toml")).unwrap(), config);
    state.disable().unwrap();
}

#[test]
fn reads_without_files_give_defaults_and_create_nothing() {
    let temp = TempDir::new().unwrap();
    let state = State::open(temp.path());

    assert_eq!(state.config().unwrap(), Config::default());
    assert_eq!(state.ignores().unwrap(), []);
    assert!(!state.is_enabled());
    assert!(!temp.path().join("info").exists());
}

#[test]
fn config_toml_gives_disabled_rules_and_options() {
    let temp = TempDir::new().unwrap();
    let state = State::open(temp.path());
    let text = "disabled = [\"no-boolean-param\"]\n\n[rules.max-nesting]\nmax = 5\n";

    write(&temp, "config.toml", text);
    let config = state.config().unwrap();
    assert_eq!(config.disabled, ["no-boolean-param"]);
    assert_eq!(config.rules["max-nesting"]["max"], toml::Value::Integer(5));

    write(
        &temp,
        "config.toml",
        &format!("\u{feff}{}", text.replace('\n', "\r\n")),
    );
    assert_eq!(state.config().unwrap(), config);

    write(&temp, "config.toml", "[rules.x]\nmax = 5\n");
    assert!(state.config().unwrap().disabled.is_empty());
}

#[test]
fn config_toml_gives_enabled_groups() {
    let temp = TempDir::new().unwrap();
    let state = State::open(temp.path());

    write(&temp, "config.toml", "enabled = [\"performance\"]\n");
    assert_eq!(state.config().unwrap().enabled, ["performance"]);

    write(&temp, "config.toml", "disabled = [\"no-any\"]\n");
    assert!(state.config().unwrap().enabled.is_empty());
}

#[test]
fn broken_config_toml_gives_the_path_and_the_line() {
    let temp = TempDir::new().unwrap();
    let state = State::open(temp.path());
    let path = folder(&temp).join("config.toml");
    assert_eq!(state.config_path(), path);
    let cases = [
        ("disabled = [\"a\"]\n\n[rules.x\n", 3, ""),
        ("\n\ndisabled = \"a\"\n", 3, ""),
        ("disabled = []\ndisable = [\"b\"]\n", 2, "`disable`"),
        ("[rules]\nx = 5\n", 2, ""),
    ];
    for (text, line, part) in cases {
        write(&temp, "config.toml", text);
        let (error_path, error_line, message) = parse_error(state.config());
        assert_eq!((&error_path, error_line), (&path, line), "{text:?}");
        assert!(!message.is_empty() && message.contains(part), "{message:?}");
    }

    write(&temp, "config.toml", cases[0].0);
    let text = state.config().unwrap_err().to_string();
    assert!(text.contains("config.toml:3: "), "{text}");
}

#[test]
fn add_ignore_appends_entries() {
    let temp = TempDir::new().unwrap();
    let state = State::open(temp.path());
    let (e1, e2, e3) = (
        entry("src/a.ts", "x:111111", "r"),
        entry("src/b.ts", "y:222222", "r"),
        entry("src/c.ts", "x:333333", "r"),
    );

    state.add_ignore(vec![e1.clone()]).unwrap();
    let text = fs::read_to_string(folder(&temp).join("ignore.json")).unwrap();
    assert_eq!(
        text,
        "[\n  {\n    \"fingerprint\": \"x:111111\",\n    \"rule\": \"x\",\n    \"file\": \"src/a.ts\",\n    \"reason\": \"r\",\n    \"added\": \"2026-10-15\"\n  }\n]\n"
    );
    assert_eq!(names(&folder(&temp)), ["ignore.json", "ignore.json.lock"]);

    // The second write renames over an existing file: a path of its own on Windows.
    state.add_ignore(vec![e2.clone(), e3.clone()]).unwrap();
    assert_eq!(state.ignores().unwrap(), [e1, e2, e3]);
    assert_eq!(names(&folder(&temp)), ["ignore.json", "ignore.json.lock"]);
}

#[test]
fn the_same_file_and_fingerprint_replace_the_entry_in_place() {
    let temp = TempDir::new().unwrap();
    let state = State::open(temp.path());
    let a = entry("src/a.ts", "x:111111", "r1");
    let b = entry("src/b.ts", "x:111111", "r1");
    let c = entry("src/a.ts", "x:222222", "r1");
    state
        .add_ignore(vec![a.clone(), b.clone(), c.clone()])
        .unwrap();

    let a2 = entry("src/a.ts", "x:111111", "r2");
    state.add_ignore(vec![a2.clone()]).unwrap();
    assert_eq!(state.ignores().unwrap(), [a2.clone(), b.clone(), c.clone()]);

    let d4 = entry("src/d.ts", "x:333333", "r4");
    state
        .add_ignore(vec![entry("src/d.ts", "x:333333", "r3"), d4.clone()])
        .unwrap();
    assert_eq!(state.ignores().unwrap(), [a2, b, c, d4]);
}

#[test]
fn broken_ignore_json_gives_the_path_and_the_line() {
    let temp = TempDir::new().unwrap();
    let state = State::open(temp.path());
    let path = folder(&temp).join("ignore.json");
    let extra = "[\n{\"fingerprint\":\"x:1\",\"rule\":\"x\",\"file\":\"a.ts\",\"reason\":\"r\",\"added\":\"d\",\n\"extra\": 1}\n]";
    let cases = [
        ("[\n  {\"fingerprint\": ", 2, ""),
        (extra, 3, "extra"),
        (
            "[\n{\"fingerprint\":\"x:1\",\n\"rule\":\"x\"}\n]",
            3,
            "`file`",
        ),
        ("{}", 1, ""),
    ];
    for (text, line, part) in cases {
        write(&temp, "ignore.json", text);
        let (error_path, error_line, message) = parse_error(state.ignores());
        assert_eq!((&error_path, error_line), (&path, line), "{text:?}");
        assert!(!message.is_empty() && message.contains(part), "{message:?}");
        assert!(!message.contains(" at line "), "{message:?}");
    }

    write(&temp, "ignore.json", cases[0].0);
    let error = parse_error(state.add_ignore(vec![entry("src/a.ts", "x:111111", "r")]));
    assert_eq!((error.0, error.1), (path.clone(), 2));
    assert_eq!(fs::read_to_string(&path).unwrap(), cases[0].0);
}

#[test]
fn ignore_json_edited_by_hand_is_read_and_rewritten_plain() {
    let temp = TempDir::new().unwrap();
    let state = State::open(temp.path());
    let e1 = entry("src/a.ts", "x:111111", "r");
    let e2 = entry("src/b.ts", "x:222222", "r");
    let one = serde_json::to_string_pretty(&[&e1]).unwrap();

    write(&temp, "ignore.json", "[]");
    assert_eq!(state.ignores().unwrap(), []);

    write(
        &temp,
        "ignore.json",
        &format!("\u{feff}{}\r\n", one.replace('\n', "\r\n")),
    );
    assert_eq!(state.ignores().unwrap(), std::slice::from_ref(&e1));

    state.add_ignore(vec![e2.clone()]).unwrap();
    let text = fs::read_to_string(folder(&temp).join("ignore.json")).unwrap();
    assert!(
        !text.starts_with('\u{feff}') && !text.contains('\r'),
        "{text:?}"
    );
    assert_eq!(state.ignores().unwrap(), [e1, e2]);
}

#[test]
fn two_ignore_at_once_lose_no_entry() {
    let temp = TempDir::new().unwrap();
    std::thread::scope(|scope| {
        for i in 0..8 {
            let path = temp.path();
            scope.spawn(move || {
                State::open(path)
                    .add_ignore(vec![entry(&format!("src/f{i}.ts"), "x:111111", "r")])
                    .unwrap();
            });
        }
    });

    let mut files: Vec<String> = State::open(temp.path())
        .ignores()
        .unwrap()
        .into_iter()
        .map(|entry| entry.file.into_string())
        .collect();
    files.sort();
    let expected: Vec<String> = (0..8).map(|i| format!("src/f{i}.ts")).collect();
    assert_eq!(files, expected);
}

#[test]
fn file_system_errors_give_the_path() {
    let temp = TempDir::new().unwrap();
    let state = State::open(temp.path());
    // EISDIR on Linux and macOS, PermissionDenied on Windows.
    fs::create_dir_all(folder(&temp).join("config.toml")).unwrap();
    assert_eq!(
        io_error_path(state.config()),
        folder(&temp).join("config.toml")
    );

    let temp = TempDir::new().unwrap();
    let state = State::open(temp.path());
    fs::create_dir_all(temp.path().join("info")).unwrap();
    fs::write(folder(&temp), "").unwrap();
    assert_eq!(io_error_path(state.enable()), folder(&temp));
    assert_eq!(
        io_error_path(state.add_ignore(vec![entry("src/a.ts", "x:111111", "r")])),
        folder(&temp)
    );
}
