use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

use testkit::TempRepo;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// By this name the run in a test process of its own knows it is that run.
const GLOBAL_CONFIG: &str = "no-identity.gitconfig";

#[test]
fn commit_needs_no_global_identity_and_runs_no_global_hooks() {
    // git takes the global config from the environment, and setting it here would reach the
    // git calls of the other tests: the test runs again in a test process of its own.
    let global = env::var_os("GIT_CONFIG_GLOBAL");
    if global.is_some_and(|path| Path::new(&path).ends_with(GLOBAL_CONFIG)) {
        let t = TempRepo::new();
        t.write("a.ts", "a\n").commit("init").branch("next");
        // The control: without the local `hooksPath` the global hooks run.
        t.git(&["config", "--unset", "core.hooksPath"]);
        t.checkout("main");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let mark = home.path().join("hooks-ran");
    let hooks = home.path().join("hooks");
    fs::create_dir(&hooks).unwrap();
    for name in ["post-commit", "post-checkout"] {
        let hook = hooks.join(name);
        fs::write(
            &hook,
            format!("#!/bin/sh\necho {name} >> '{}'\n", slashed(&mark)),
        )
        .unwrap();
        #[cfg(unix)]
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let config = home.path().join(GLOBAL_CONFIG);
    // With `useConfigOnly` and no name git refuses to commit instead of making one up from
    // the user and the host.
    let text = format!(
        "[user]\n\tuseConfigOnly = true\n[core]\n\thooksPath = {}\n",
        slashed(&hooks)
    );
    fs::write(&config, text).unwrap();

    let mut command = Command::new(env::current_exe().unwrap());
    for name in [
        "GIT_AUTHOR_NAME",
        "GIT_AUTHOR_EMAIL",
        "GIT_COMMITTER_NAME",
        "GIT_COMMITTER_EMAIL",
        "EMAIL",
    ] {
        command.env_remove(name);
    }
    let out = command
        .args([
            "--exact",
            "commit_needs_no_global_identity_and_runs_no_global_hooks",
        ])
        .env("GIT_CONFIG_GLOBAL", &config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("1 passed"), "{stdout}");
    // Only the control's checkout. No line at all would mean the hooks cannot run here, and
    // the test would prove nothing.
    let ran = fs::read_to_string(&mark).unwrap_or_default();
    assert_eq!(ran.trim_end(), "post-checkout", "{ran}");
}

/// `\` is an escape in a git config value; git and its `sh` on Windows take `C:/...`.
fn slashed(path: &Path) -> String {
    path.to_str().unwrap().replace('\\', "/")
}
