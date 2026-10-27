use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use gitcmd::{Git, GitError, root};
use testkit::TempRepo;

/// Neither exists: a `GIT_DIR` that reached git would fail every call, and a `GIT_INDEX_FILE`
/// would empty the index.
const DECOY_GIT_DIR: &str = "no-such-git-dir";
const DECOY_INDEX: &str = "no-such-index";

/// On macOS temp folders sit behind `/var` -> `/private/var`, and Windows CI gives short
/// names like `RUNNER~1`: paths are compared in canonical form.
fn canon(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn the_git_variables_of_the_process_do_not_reach_git_but_env_does() {
    // Setting them here would reach the git calls of the other tests: the test runs again in
    // a test process of its own.
    if env::var_os("GIT_DIR").is_none_or(|dir| dir != DECOY_GIT_DIR) {
        let out = Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "the_git_variables_of_the_process_do_not_reach_git_but_env_does",
            ])
            .env("GIT_DIR", DECOY_GIT_DIR)
            .env("GIT_INDEX_FILE", DECOY_INDEX)
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
    let t = TempRepo::new();
    t.write("a.ts", "a\n").commit("init");

    assert_eq!(Git::at(t.path()).run(&["ls-files"]).unwrap(), b"a.ts\n");
    let other_index = t.path().join(".git/other-index");
    let git = Git::at(t.path()).env("GIT_INDEX_FILE", &other_index);
    assert_eq!(git.run(&["ls-files"]).unwrap(), b"");
}

#[test]
fn run_found_is_none_on_exit_code_1_and_failed_on_another() {
    let t = TempRepo::new();
    t.write("a.ts", "a\n").commit("init");
    let git = Git::at(t.path());
    let head = format!("{}\n", t.git(&["rev-parse", "HEAD"]));
    let quiet = ["rev-parse", "--verify", "--quiet", "no-such"];

    let found = git.run_found(&["rev-parse", "--verify", "--quiet", "HEAD"]);
    assert_eq!(found.unwrap(), Some(head.into_bytes()));
    assert_eq!(git.run_found(&quiet).unwrap(), None);
    assert!(matches!(git.run(&quiet), Err(GitError::Failed { .. })));
    match git.run_found(&["rev-parse", "--verify", "no-such"]) {
        Err(GitError::Failed { command, stderr }) => {
            assert_eq!(command, "git rev-parse --verify no-such");
            assert!(stderr.starts_with("fatal: "), "{stderr}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn root_gives_the_work_tree_and_the_common_dir_from_a_subfolder_and_a_linked_worktree() {
    let t = TempRepo::new();
    t.write("src/a.ts", "a\n").commit("init");
    let wt = t.path().join("wt");
    t.git(&["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()]);

    let from_src = root(&t.path().join("src")).unwrap().expect("a repository");
    let from_wt = root(&wt.join("src")).unwrap().expect("a repository");

    let common = canon(&t.path().join(".git"));
    for found in [&from_src, &from_wt] {
        assert!(found.worktree.is_absolute(), "{found:?}");
        assert!(found.common.is_absolute(), "{found:?}");
        assert_eq!(canon(&found.common), common);
    }
    assert_eq!(canon(&from_src.worktree), canon(t.path()));
    assert_eq!(canon(&from_wt.worktree), canon(&wt));
    assert_eq!(root(&env::temp_dir()).unwrap(), None);
}

#[test]
fn a_long_input_does_not_hang_git() {
    let t = TempRepo::new();
    t.write("a.ts", "a\n").commit("init");
    // Far over a pipe buffer both ways, and git answers each line while the rest still comes.
    let requests = format!("{}\n", t.git(&["rev-parse", "HEAD:a.ts"])).repeat(5_000);

    let out = Git::at(t.path())
        .run_with_input(&["cat-file", "--batch-check"], requests.as_bytes())
        .unwrap();

    assert_eq!(String::from_utf8(out).unwrap().lines().count(), 5_000);
}
