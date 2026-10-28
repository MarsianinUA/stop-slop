//! Each test starts `node`, so `node` must be in `PATH`; CI installs Node 22.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::process::Command;

use camino::{Utf8Path, Utf8PathBuf};
use profile::{Exited, ProfileError, read, run};
use serde_json::Value;
use tempfile::TempDir;

/// The profile folder in a temp folder. Its path has a space, and on Windows `\`, which
/// `NODE_OPTIONS` reads as an escape inside `"`.
fn folder() -> (TempDir, Utf8PathBuf) {
    let temp = tempfile::tempdir().expect("create a temp folder");
    let dir = Utf8PathBuf::try_from(temp.path().join("a profile")).expect("a UTF-8 temp folder");
    fs::create_dir_all(&dir).unwrap();
    (temp, dir)
}

/// Next to the profile folder, not in it.
fn script(temp: &TempDir, name: &str, text: &str) -> OsString {
    let path = temp.path().join(name);
    fs::write(&path, text).unwrap();
    path.into_os_string()
}

fn node(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut command = vec![OsString::from("node")];
    command.extend(args);
    command
}

fn names(dir: &Utf8Path) -> Vec<String> {
    let mut names: Vec<String> = dir
        .read_dir_utf8()
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_owned())
        .collect();
    names.sort();
    names
}

/// The names without the process id: `start.json`, `0.cpuprofile`.
fn kinds(dir: &Utf8Path) -> Vec<String> {
    names(dir)
        .into_iter()
        .filter(|name| name != "preload.cjs")
        .map(|name| name.split_once('.').expect("<id>.<kind>").1.to_owned())
        .collect()
}

fn json(dir: &Utf8Path, suffix: &str) -> Value {
    let name = names(dir)
        .into_iter()
        .find(|name| name.ends_with(suffix))
        .unwrap_or_else(|| panic!("no {suffix} file in {dir}"));
    serde_json::from_slice(&fs::read(dir.join(name)).unwrap()).unwrap()
}

#[test]
fn a_node_script_writes_a_start_mark_and_a_chunk_and_passes_on_its_exit_code() {
    let (temp, dir) = folder();
    let exit = script(&temp, "exit.js", "process.exitCode = 7;\n");
    assert_eq!(run(&node([exit]), &dir).unwrap(), Exited::Code(7));
    assert_eq!(kinds(&dir), ["0.cpuprofile", "start.json"]);
    assert!(json(&dir, ".cpuprofile")["nodes"].is_array());
}

#[test]
fn chunks_written_before_a_hard_kill_stay() {
    let (temp, dir) = folder();
    let kill = script(
        &temp,
        "kill.js",
        "const fs = require('node:fs');\n\
         const dir = process.env.STOP_SLOP_PROFILE_DIR;\n\
         setInterval(() => {\n\
         \x20 const chunks = fs.readdirSync(dir).filter((name) => name.endsWith('.cpuprofile'));\n\
         \x20 if (chunks.length === 2) process.kill(process.pid, 'SIGKILL');\n\
         }, 50);\n",
    );
    let exit = run(&node([kill]), &dir).unwrap();
    // On Windows the kill is `TerminateProcess` with exit code 1.
    let killed = if cfg!(windows) {
        Exited::Code(1)
    } else {
        Exited::Signal
    };
    assert_eq!(exit, killed);
    // The two chunks of the 2 s timer and none of `exit`, which a kill does not run.
    assert_eq!(kinds(&dir), ["0.cpuprofile", "1.cpuprofile", "start.json"]);
    let samples = read(&dir, &dir).unwrap();
    assert_eq!((samples.processes, samples.lost_processes), (1, 0));
    assert!(samples.total_ms > 3000.0, "{}", samples.total_ms);
}

#[test]
fn a_pid_an_earlier_process_had_gets_files_of_its_own() {
    // The preload loaded a second time in one process is what a later process with the same
    // pid looks like.
    let (temp, dir) = folder();
    let again = script(
        &temp,
        "again.js",
        "const preload = require('node:path').join(process.env.STOP_SLOP_PROFILE_DIR, 'preload.cjs');\n\
         delete require.cache[require.resolve(preload)];\n\
         require(preload);\n",
    );
    assert_eq!(run(&node([again]), &dir).unwrap(), Exited::Code(0));
    let names = names(&dir);
    let pid: String = names[0].chars().take_while(char::is_ascii_digit).collect();
    assert_eq!(
        names,
        [
            format!("{pid}-r1.0.cpuprofile"),
            format!("{pid}-r1.start.json"),
            format!("{pid}.0.cpuprofile"),
            format!("{pid}.start.json"),
            "preload.cjs".to_owned(),
        ]
    );
    assert_eq!(read(&dir, &dir).unwrap().processes, 2);
}

#[test]
fn a_worker_thread_gets_files_of_its_own() {
    let (temp, dir) = folder();
    let worker = script(
        &temp,
        "worker.js",
        "new (require('node:worker_threads').Worker)('', { eval: true });\n",
    );
    assert_eq!(run(&node([worker]), &dir).unwrap(), Exited::Code(0));
    let names = names(&dir);
    let pid: String = names[0].chars().take_while(char::is_ascii_digit).collect();
    assert_eq!(
        names,
        [
            format!("{pid}-t1.0.cpuprofile"),
            format!("{pid}-t1.start.json"),
            format!("{pid}.0.cpuprofile"),
            format!("{pid}.start.json"),
            "preload.cjs".to_owned(),
        ]
    );
    assert_eq!(read(&dir, &dir).unwrap().processes, 2);
}

#[test]
fn the_previous_node_options_are_kept() {
    // `run` passes on `NODE_OPTIONS` of this process, and setting it here would reach the
    // commands of the other tests: the test runs again in a test process of its own.
    if !env::var("NODE_OPTIONS").is_ok_and(|options| options.contains("--no-deprecation")) {
        let out = Command::new(env::current_exe().unwrap())
            .args(["--exact", "the_previous_node_options_are_kept"])
            .env("NODE_OPTIONS", "--no-deprecation")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(stdout.contains("1 passed"), "{stdout}");
        return;
    }
    let (temp, dir) = folder();
    let check = script(
        &temp,
        "check.js",
        "process.exitCode = process.noDeprecation ? 0 : 5;\n",
    );
    assert_eq!(run(&node([check]), &dir).unwrap(), Exited::Code(0));
    assert_eq!(kinds(&dir), ["0.cpuprofile", "start.json"]);
}

#[test]
fn the_exit_code_and_error_stack_are_the_same_as_without_the_preload() {
    let (temp, dir) = folder();
    let fail = script(
        &temp,
        "fail.js",
        "function fail() {\n\
         \x20 return new Error('boom');\n\
         }\n\
         // The preload turns the debugger on: these must neither stop nor slow the program.\n\
         // They take milliseconds without the preload and 8 s when each one pauses.\n\
         const start = Date.now();\n\
         for (let i = 0; i < 20000; i++) debugger;\n\
         if (Date.now() - start > 1000) process.exit(4);\n\
         require('node:fs').writeFileSync(process.argv[2], fail().stack);\n\
         process.exit(3);\n",
    );
    let with = temp.path().join("with.txt");
    let without = temp.path().join("without.txt");
    let exit = run(&node([fail.clone(), with.clone().into()]), &dir).unwrap();
    let status = Command::new("node")
        .arg(&fail)
        .arg(&without)
        .status()
        .unwrap();
    assert_eq!((exit, status.code()), (Exited::Code(3), Some(3)));
    assert_eq!(
        fs::read_to_string(with).unwrap(),
        fs::read_to_string(without).unwrap()
    );
    assert_eq!(kinds(&dir), ["0.cpuprofile", "start.json"]);
}

#[test]
fn code_compiled_with_vm_compile_function_gets_its_inline_map() {
    let (temp, dir) = folder();
    let source = temp.path().join("add.ts");
    let compile = script(
        &temp,
        "compile.js",
        "const source = process.argv[2];\n\
         const map = { version: 3, sources: [source], sourcesContent: ['x'], names: [], mappings: 'AAAA;AACA' };\n\
         const code = 'const end = Date.now() + 100;\\nwhile (Date.now() < end);\\n' +\n\
         \x20 '//# sourceMappingURL=data:application/json;base64,' + Buffer.from(JSON.stringify(map)).toString('base64');\n\
         require('node:vm').compileFunction(code, [], { filename: source })();\n",
    );
    assert_eq!(
        run(&node([compile, source.clone().into()]), &dir).unwrap(),
        Exited::Code(0)
    );
    assert_eq!(kinds(&dir), ["0.cpuprofile", "0.maps.json", "start.json"]);
    let maps = json(&dir, ".maps.json");
    let (url, entry) = maps
        .as_object()
        .unwrap()
        .iter()
        .find(|(url, _)| url.ends_with("/add.ts"))
        .unwrap_or_else(|| panic!("no map of add.ts in {maps}"));
    assert!(url.starts_with("file:///"), "{url}");
    let map = &entry["map"];
    assert_eq!(map["sources"][0], source.to_str().unwrap());
    assert_eq!(map["mappings"], "AAAA;AACA");
    assert!(map.get("sourcesContent").is_none(), "{map}");
}

#[test]
fn a_program_that_does_not_exist_is_a_spawn_error() {
    let (_temp, dir) = folder();
    let command = [OsString::from("stop-slop-no-such-program")];
    match run(&command, &dir) {
        Err(ProfileError::Spawn { program, .. }) => {
            assert_eq!(program, "stop-slop-no-such-program");
        }
        other => panic!("expected a spawn error, got {other:?}"),
    }
}

/// `npm`, `pnpm` and `node_modules/.bin/jest` are `.cmd` files on Windows; `node.exe` is found
/// through `PATH` in every test.
#[cfg(windows)]
#[test]
fn a_cmd_file_runs_without_its_extension_and_with_its_arguments() {
    let (temp, dir) = folder();
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("tool.cmd"), "@node \"%~dp0args.js\" %*\r\n").unwrap();
    fs::write(
        bin.join("args.js"),
        "require('node:fs').writeFileSync(__dirname + '/args.json', JSON.stringify(process.argv.slice(2)));\n",
    )
    .unwrap();
    let command = [bin.join("tool").into(), "a b".into(), "c&d".into()];
    assert_eq!(run(&command, &dir).unwrap(), Exited::Code(0));
    let args = fs::read_to_string(bin.join("args.json")).unwrap();
    assert_eq!(args, r#"["a b","c&d"]"#);
    assert_eq!(kinds(&dir), ["0.cpuprofile", "start.json"]);
}
