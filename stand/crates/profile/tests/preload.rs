//! Each test starts `node`, so `node` must be in `PATH`; CI installs Node 22.

use std::fs;
use std::process::{Command, ExitStatus};

use camino::{Utf8Path, Utf8PathBuf};
use profile::{NodeEnv, PRELOAD, PathMap, ProfileMode, Samples, node_env, read};
use serde_json::Value;
use tempfile::TempDir;

/// In a temp folder, the preload folder with `PRELOAD` in it, as the stand writes it, and the
/// empty profile folder. Their paths have a space, and on Windows `\`, which `NODE_OPTIONS`
/// reads as an escape inside `"`. The preload's has a `~`, as a Windows short name such as
/// `RUNNER~1` does: `pathToFileURL` writes it `%7E`, V8's script URLs keep it.
struct Folders {
    _temp: TempDir,
    dir: Utf8PathBuf,
    preload: Utf8PathBuf,
    profile: Utf8PathBuf,
}

impl Folders {
    fn new() -> Folders {
        let temp = tempfile::tempdir().expect("create a temp folder");
        let dir = Utf8PathBuf::try_from(temp.path().to_owned()).expect("a UTF-8 temp folder");
        let preload = dir.join("the preload~1");
        let profile = dir.join("the profile");
        fs::create_dir(&preload).unwrap();
        fs::create_dir(&profile).unwrap();
        fs::write(preload.join("preload.cjs"), PRELOAD).unwrap();
        Folders {
            _temp: temp,
            dir,
            preload,
            profile,
        }
    }

    /// In the temp folder, next to the two others.
    fn script(&self, name: &str, text: &str) -> Utf8PathBuf {
        let path = self.dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    /// `node <args>` in the temp folder, with the variables of `node_env`.
    fn node(&self, mode: ProfileMode, inherited: Option<&str>, args: &[&str]) -> ExitStatus {
        let env = node_env(
            self.preload.as_str(),
            self.profile.as_str(),
            inherited,
            mode,
        );
        Command::new("node")
            .args(args)
            .current_dir(&self.dir)
            .envs(env.into_vars())
            .status()
            .expect("start node")
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .profile
            .read_dir_utf8()
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_owned())
            .collect();
        names.sort();
        names
    }

    /// The names without the process id: `start.json`, `0.cpuprofile`.
    fn kinds(&self) -> Vec<String> {
        self.names()
            .into_iter()
            .map(|name| name.split_once('.').expect("<id>.<kind>").1.to_owned())
            .collect()
    }

    fn json(&self, suffix: &str) -> Value {
        let name = self
            .names()
            .into_iter()
            .find(|name| name.ends_with(suffix))
            .unwrap_or_else(|| panic!("no {suffix} file in {}", self.profile));
        serde_json::from_slice(&fs::read(self.profile.join(name)).unwrap()).unwrap()
    }

    /// The scripts run on the host, outside the guest tree: none is local, the counts hold.
    fn read(&self) -> Samples {
        read(&self.profile, &PathMap::new("/w/app")).unwrap()
    }
}

#[test]
fn node_env_puts_the_preload_first_and_names_the_folder_and_the_mode() {
    assert_eq!(
        node_env(
            "/stand/preload",
            "/stand/profile",
            Some("--max-old-space-size=4096"),
            ProfileMode::Cpu,
        ),
        NodeEnv {
            node_options: r#"--require "/stand/preload/preload.cjs" --max-old-space-size=4096"#
                .to_owned(),
            profile_dir: "/stand/profile".to_owned(),
            profile_mode: "cpu",
        }
    );
    assert_eq!(
        node_env(r#"C:\a "b""#, "/p", None, ProfileMode::Coverage).into_vars(),
        [
            (
                "NODE_OPTIONS",
                r#"--require "C:\\a \"b\"/preload.cjs""#.to_owned()
            ),
            ("STOP_SLOP_PROFILE_DIR", "/p".to_owned()),
            ("STOP_SLOP_PROFILE_MODE", "coverage".to_owned()),
        ]
    );
}

#[test]
fn a_node_script_writes_a_start_mark_and_a_chunk_and_keeps_its_exit_code() {
    let folders = Folders::new();
    let exit = folders.script("exit.js", "process.exitCode = 7;\n");
    let status = folders.node(ProfileMode::Cpu, None, &[exit.as_str()]);
    assert_eq!(status.code(), Some(7));
    assert_eq!(folders.kinds(), ["0.cpuprofile", "start.json"]);
    assert!(folders.json(".cpuprofile")["nodes"].is_array());
}

#[test]
fn chunks_written_before_a_hard_kill_stay() {
    let folders = Folders::new();
    let kill = folders.script(
        "kill.js",
        "const fs = require('node:fs');\n\
         const dir = process.env.STOP_SLOP_PROFILE_DIR;\n\
         setInterval(() => {\n\
         \x20 const chunks = fs.readdirSync(dir).filter((name) => name.endsWith('.cpuprofile'));\n\
         \x20 if (chunks.length === 2) process.kill(process.pid, 'SIGKILL');\n\
         }, 50);\n",
    );
    let status = folders.node(ProfileMode::Cpu, None, &[kill.as_str()]);
    // On Windows the kill is `TerminateProcess` with exit code 1; elsewhere a signal, no code.
    assert_eq!(status.code(), cfg!(windows).then_some(1));
    // The two chunks of the 2 s timer and none of `exit`, which a kill does not run.
    assert_eq!(
        folders.kinds(),
        ["0.cpuprofile", "1.cpuprofile", "start.json"]
    );
    let samples = folders.read();
    assert_eq!((samples.processes, samples.lost_processes), (1, 0));
    assert!(samples.total_ms > 3000.0, "{}", samples.total_ms);
}

#[test]
fn a_pid_an_earlier_process_had_gets_files_of_its_own() {
    // The preload loaded a second time in one process is what a later process with the same
    // pid looks like.
    let folders = Folders::new();
    let again = folders.script(
        "again.js",
        "const preload = process.argv[2];\n\
         delete require.cache[require.resolve(preload)];\n\
         require(preload);\n",
    );
    let preload = folders.preload.join("preload.cjs");
    let status = folders.node(ProfileMode::Cpu, None, &[again.as_str(), preload.as_str()]);
    assert_eq!(status.code(), Some(0));
    let names = folders.names();
    let pid: String = names[0].chars().take_while(char::is_ascii_digit).collect();
    assert_eq!(
        names,
        [
            format!("{pid}-r1.0.cpuprofile"),
            format!("{pid}-r1.start.json"),
            format!("{pid}.0.cpuprofile"),
            format!("{pid}.start.json"),
        ]
    );
    assert_eq!(folders.read().processes, 2);
}

#[test]
fn a_worker_thread_gets_files_of_its_own() {
    let folders = Folders::new();
    let worker = folders.script(
        "worker.js",
        "new (require('node:worker_threads').Worker)('', { eval: true });\n",
    );
    let status = folders.node(ProfileMode::Cpu, None, &[worker.as_str()]);
    assert_eq!(status.code(), Some(0));
    let names = folders.names();
    let pid: String = names[0].chars().take_while(char::is_ascii_digit).collect();
    assert_eq!(
        names,
        [
            format!("{pid}-t1.0.cpuprofile"),
            format!("{pid}-t1.start.json"),
            format!("{pid}.0.cpuprofile"),
            format!("{pid}.start.json"),
        ]
    );
    assert_eq!(folders.read().processes, 2);
}

#[test]
fn the_services_own_node_options_run_after_the_preload() {
    let folders = Folders::new();
    // Node resolves `--require` from the working folder, the temp folder of the scripts.
    folders.script(
        "user.cjs",
        "const fs = require('node:fs');\n\
         const marks = fs.readdirSync(process.env.STOP_SLOP_PROFILE_DIR);\n\
         if (!marks.some((name) => name.endsWith('.start.json'))) process.exit(5);\n",
    );
    let check = folders.script(
        "check.js",
        "process.exitCode = process.noDeprecation ? 0 : 6;\n",
    );
    let inherited = "--no-deprecation --require ./user.cjs";
    let status = folders.node(ProfileMode::Cpu, Some(inherited), &[check.as_str()]);
    assert_eq!(status.code(), Some(0));
    assert_eq!(folders.kinds(), ["0.cpuprofile", "start.json"]);
}

#[test]
fn the_exit_code_and_error_stack_are_the_same_as_without_the_preload() {
    for (mode, chunk) in [
        (ProfileMode::Cpu, "0.cpuprofile"),
        (ProfileMode::Coverage, "0.coverage.json"),
    ] {
        let folders = Folders::new();
        let fail = folders.script(
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
        let with = folders.dir.join("with.txt");
        let without = folders.dir.join("without.txt");
        let status = folders.node(mode, None, &[fail.as_str(), with.as_str()]);
        let plain = Command::new("node")
            .arg(&fail)
            .arg(&without)
            .status()
            .unwrap();
        assert_eq!(
            (status.code(), plain.code()),
            (Some(3), Some(3)),
            "{mode:?}"
        );
        assert_eq!(
            fs::read_to_string(with).unwrap(),
            fs::read_to_string(without).unwrap(),
            "{mode:?}"
        );
        assert_eq!(folders.kinds(), [chunk, "start.json"]);
    }
}

#[test]
fn coverage_writes_the_calls_of_each_function_with_the_line_and_column_of_its_start() {
    let folders = Folders::new();
    folders.script("node_modules/dep/index.js", "exports.run = () => 1;\n");
    // As in V8's own positions, a line end of Windows counts as one and a column counts UTF-16
    // units: `é` is one, `😀` two.
    let count = folders.script(
        "count.js",
        "function add(a, b) {\r\n\
         \x20 return a + b;\r\n\
         }\r\n\
         const twice = /* é😀 */ (x) => x * 2;\r\n\
         function unused() {}\r\n\
         class Box { take(y) { return y; } }\r\n\
         for (let i = 0; i < 3; i++) add(i, i);\r\n\
         twice(1);\r\n\
         new Box().take(2);\r\n\
         require('dep').run();\r\n",
    );
    let status = folders.node(ProfileMode::Coverage, None, &[count.as_str()]);
    assert_eq!(status.code(), Some(0));
    assert_eq!(folders.kinds(), ["0.coverage.json", "start.json"]);
    let scripts = folders.json(".coverage.json");
    let urls: Vec<&str> = scripts
        .as_array()
        .unwrap()
        .iter()
        .map(|script| script["url"].as_str().unwrap())
        .collect();
    // Neither `node:` modules, `node_modules` nor the preload itself.
    assert!(
        urls.len() == 1 && urls[0].starts_with("file:///") && urls[0].ends_with("/count.js"),
        "{urls:?}"
    );
    let functions: Vec<(&str, u64, u64, u64)> = scripts[0]["functions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|function| {
            (
                function["functionName"].as_str().unwrap(),
                function["line"].as_u64().unwrap(),
                function["column"].as_u64().unwrap(),
                function["ranges"][0]["count"].as_u64().unwrap(),
            )
        })
        .collect();
    // V8 starts a function declaration at its `function`, an arrow function at its parameters,
    // a method at its name.
    assert_eq!(
        functions,
        [
            ("", 0, 0, 1),
            ("add", 0, 0, 3),
            ("twice", 3, 24, 1),
            ("unused", 4, 0, 0),
            ("take", 5, 12, 1),
        ]
    );
}

#[test]
fn a_map_file_next_to_the_script_goes_into_maps_json_without_sources_content() {
    let folders = Folders::new();
    // The space puts `%20` into the script's URL, which the map's name is resolved against.
    // `gone.js` names a map that is not there: only that script goes without one.
    folders.script(
        "a dist/gone.js",
        "exports.go = () => 1;\n//# sourceMappingURL=gone.js.map\n",
    );
    let app = folders.script(
        "a dist/app.js",
        "function main() {}\nmain();\nrequire('./gone.js').go();\n//# sourceMappingURL=app.js.map\n",
    );
    folders.script(
        "a dist/app.js.map",
        r#"{"version":3,"sources":["../src/app.ts"],"sourcesContent":["x"],"names":[],"mappings":"AAAA"}"#,
    );
    let status = folders.node(ProfileMode::Coverage, None, &[app.as_str()]);
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        folders.kinds(),
        ["0.coverage.json", "0.maps.json", "start.json"]
    );
    let maps = folders.json(".maps.json");
    let [(url, entry)] = maps.as_object().unwrap().iter().collect::<Vec<_>>()[..] else {
        panic!("not one map in {maps}");
    };
    assert!(url.ends_with("/a%20dist/app.js"), "{url}");
    assert_eq!(
        entry["map"],
        serde_json::json!({"version": 3, "sources": ["../src/app.ts"], "names": [], "mappings": "AAAA"})
    );
    // The path the relative source starts from, as the guest sees it.
    let base = Utf8Path::new(entry["base"].as_str().unwrap());
    assert!(
        base.ends_with("a dist/app.js.map") && base.is_file(),
        "{base}"
    );
}

#[test]
fn code_compiled_with_vm_compile_function_gets_its_inline_map() {
    let folders = Folders::new();
    let source = folders.dir.join("add.ts");
    let compile = folders.script(
        "compile.js",
        "const source = process.argv[2];\n\
         const map = { version: 3, sources: [source], sourcesContent: ['x'], names: [], mappings: 'AAAA;AACA' };\n\
         const code = 'const end = Date.now() + 100;\\nwhile (Date.now() < end);\\n' +\n\
         \x20 '//# sourceMappingURL=data:application/json;base64,' + Buffer.from(JSON.stringify(map)).toString('base64');\n\
         require('node:vm').compileFunction(code, [], { filename: source })();\n",
    );
    let status = folders.node(ProfileMode::Cpu, None, &[compile.as_str(), source.as_str()]);
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        folders.kinds(),
        ["0.cpuprofile", "0.maps.json", "start.json"]
    );
    let maps = folders.json(".maps.json");
    let (url, entry) = maps
        .as_object()
        .unwrap()
        .iter()
        .find(|(url, _)| url.ends_with("/add.ts"))
        .unwrap_or_else(|| panic!("no map of add.ts in {maps}"));
    assert!(url.starts_with("file:///"), "{url}");
    let map = &entry["map"];
    assert_eq!(map["sources"][0], source.as_str());
    assert_eq!(map["mappings"], "AAAA;AACA");
    assert_eq!(entry["base"], source.as_str());
    assert!(map.get("sourcesContent").is_none(), "{map}");
}
