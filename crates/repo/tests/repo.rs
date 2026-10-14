use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use camino::{Utf8Path, Utf8PathBuf};
use repo::{BranchTip, Oid, Repo, RepoError, Snapshot};
use testkit::TempRepo;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// On macOS temp folders sit behind `/var` -> `/private/var`, and Windows CI gives short
/// names like `RUNNER~1`: paths are compared in canonical form.
fn canon(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[cfg(unix)]
fn symlink_dir_or_skip(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).unwrap();
    true
}

/// A symlink on Windows needs admin rights or Developer Mode; without them the case is
/// skipped. The GitHub Actions runner is admin, so CI runs it.
#[cfg(windows)]
fn symlink_dir_or_skip(target: &Path, link: &Path) -> bool {
    const ERROR_PRIVILEGE_NOT_HELD: i32 = 1314;
    match std::os::windows::fs::symlink_dir(target, link) {
        Ok(()) => true,
        Err(error) if error.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD) => {
            eprintln!("skipped: no right to create a symlink ({error})");
            false
        }
        Err(error) => panic!("symlink {}: {error}", link.display()),
    }
}

fn open(t: &TempRepo) -> Repo {
    Repo::open(t.path()).unwrap()
}

fn paths(names: &[&str]) -> Vec<Utf8PathBuf> {
    names.iter().copied().map(Utf8PathBuf::from).collect()
}

fn oid_of(found: Result<Option<Oid>, RepoError>) -> String {
    found.unwrap().expect("a commit").to_string()
}

/// `origin/HEAD` points at `origin/main`, as a clone leaves it.
fn set_origin_main(t: &TempRepo, oid: &str) {
    t.git(&["update-ref", "refs/remotes/origin/main", oid]);
    t.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
}

#[test]
fn open_finds_one_common_dir_from_a_subfolder_and_a_worktree() {
    let t = TempRepo::new();
    t.write("src/a.ts", "a\n").commit("init");
    let elsewhere = tempfile::tempdir().unwrap();
    let wt = elsewhere.path().join("wt");
    t.git(&["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()]);

    let from_src = Repo::open(&t.path().join("src")).unwrap();
    let from_wt = Repo::open(&wt.join("src")).unwrap();

    let common = canon(&t.path().join(".git"));
    assert!(from_src.git_common_dir().is_absolute());
    assert!(from_wt.git_common_dir().is_absolute());
    assert_eq!(canon(from_src.git_common_dir()), common);
    assert_eq!(canon(from_wt.git_common_dir()), common);
    assert_eq!(canon(from_src.root()), canon(t.path()));
    assert_eq!(canon(from_wt.root()), canon(&wt));

    let outside = tempfile::tempdir().unwrap();
    assert!(matches!(
        Repo::open(outside.path()),
        Err(RepoError::NotARepo)
    ));
}

#[test]
fn relativize_takes_an_absolute_path_and_a_path_from_cwd() {
    let t = TempRepo::new();
    t.write("src/a.ts", "a\n").write("b.ts", "b\n");

    assert_eq!(
        open(&t).relativize(&t.path().join("src/a.ts")).unwrap(),
        "src/a.ts"
    );
    let from_src = Repo::open(&t.path().join("src")).unwrap();
    assert_eq!(from_src.relativize(Path::new("a.ts")).unwrap(), "src/a.ts");
    assert_eq!(from_src.relativize(Path::new("../b.ts")).unwrap(), "b.ts");
}

#[test]
fn relativize_resolves_a_symlink_on_both_sides() {
    let t = TempRepo::new();
    t.write("src/a.ts", "a\n");
    let links = tempfile::tempdir().unwrap();
    let alias = links.path().join("alias");
    if !symlink_dir_or_skip(t.path(), &alias) {
        return;
    }

    let via_alias = Repo::open(&alias.join("src")).unwrap();
    assert_eq!(
        via_alias.relativize(&t.path().join("src/a.ts")).unwrap(),
        "src/a.ts"
    );
    assert_eq!(
        open(&t).relativize(&alias.join("src/a.ts")).unwrap(),
        "src/a.ts"
    );
}

#[cfg(windows)]
#[test]
fn relativize_turns_backslashes_into_slashes_on_windows() {
    let t = TempRepo::new();
    t.write("src/a.ts", "a\n");
    let repo = open(&t);

    assert_eq!(repo.relativize(Path::new(r"src\a.ts")).unwrap(), "src/a.ts");
    let absolute = format!(r"{}\src\a.ts", t.path().display());
    assert_eq!(repo.relativize(Path::new(&absolute)).unwrap(), "src/a.ts");
}

#[cfg(unix)]
#[test]
fn relativize_keeps_a_backslash_in_a_name_on_unix() {
    let t = TempRepo::new();
    t.write(r"a\b.ts", "a\n");

    assert_eq!(
        open(&t).relativize(&t.path().join(r"a\b.ts")).unwrap(),
        r"a\b.ts"
    );
}

#[test]
fn relativize_rejects_a_path_outside_the_repo_and_a_missing_file() {
    let t = TempRepo::new();
    let repo = open(&t);
    let outside = tempfile::tempdir().unwrap();
    let stranger = outside.path().join("x.ts");
    fs::write(&stranger, "x\n").unwrap();

    match repo.relativize(&stranger) {
        Err(RepoError::OutsideRepo(path)) => assert_eq!(path, stranger),
        other => panic!("{other:?}"),
    }
    match repo.relativize(Path::new("src/nope.ts")) {
        Err(RepoError::Io { source, .. }) => assert_eq!(source.kind(), ErrorKind::NotFound),
        other => panic!("{other:?}"),
    }
}

#[test]
fn branches_lists_local_and_origin_branches() {
    let t = TempRepo::new();
    t.write("a.ts", "a\n").commit("init");
    t.branch("feat").write("b.ts", "b\n").commit("feat");
    let main = t.git(&["rev-parse", "main"]);
    let feat = t.git(&["rev-parse", "feat"]);
    set_origin_main(&t, &main);
    t.git(&["update-ref", "refs/remotes/origin/feat", &feat]);
    t.git(&["update-ref", "refs/remotes/upstream/x", &main]);
    t.git(&["tag", "v1"]);

    let tips: Vec<(String, String)> = open(&t)
        .branches()
        .unwrap()
        .into_iter()
        .map(|tip| (tip.name, tip.oid.to_string()))
        .collect();

    let tip = |name: &str, oid: &str| (name.to_owned(), oid.to_owned());
    assert_eq!(
        tips,
        [
            tip("feat", &feat),
            tip("main", &main),
            tip("origin/feat", &feat),
            tip("origin/main", &main),
        ]
    );
}

#[test]
fn current_and_default_branch() {
    let t = TempRepo::new();
    t.write("a.ts", "a\n").commit("init").branch("feat");
    let repo = open(&t);

    assert_eq!(repo.current_branch().unwrap().as_deref(), Some("feat"));
    t.git(&["checkout", "-q", "--detach"]);
    assert_eq!(repo.current_branch().unwrap(), None);

    assert_eq!(repo.default_branch().unwrap(), None);
    let main = repo.resolve("main").unwrap().unwrap();
    set_origin_main(&t, &main.to_string());
    assert_eq!(
        repo.default_branch().unwrap(),
        Some(BranchTip {
            name: "origin/main".to_owned(),
            oid: main,
        })
    );
}

#[test]
fn resolve_and_merge_base() {
    let t = TempRepo::new();
    t.write("a.ts", "a\n").commit("fork");
    let fork = t.git(&["rev-parse", "HEAD"]);
    t.git(&["tag", "-a", "v1", "-m", "v1"]);
    t.branch("feat").write("b.ts", "b\n").commit("feat");
    t.checkout("main").write("c.ts", "c\n").commit("main");
    let repo = open(&t);

    assert_eq!(oid_of(repo.resolve("feat")), t.git(&["rev-parse", "feat"]));
    assert_eq!(oid_of(repo.resolve("v1")), fork);
    assert_ne!(fork, t.git(&["rev-parse", "v1"]));
    assert_eq!(repo.resolve("no-such").unwrap(), None);
    assert_eq!(repo.resolve("-x").unwrap(), None);

    let feat = repo.resolve("feat").unwrap().unwrap();
    let main = repo.resolve("main").unwrap().unwrap();
    assert_eq!(oid_of(repo.merge_base(&feat, &main)), fork);

    t.git(&["checkout", "-q", "--orphan", "lone"]);
    t.commit("lone");
    let lone = repo.resolve("lone").unwrap().unwrap();
    assert_eq!(repo.merge_base(&lone, &main).unwrap(), None);
}

#[test]
fn read_gives_the_bytes_of_the_revision_or_of_the_disk() {
    let t = TempRepo::new();
    t.write("src/a.ts", "old\n")
        .write("src/crlf.ts", "a\r\nb\r\n")
        .commit("init");
    t.write("src/a.ts", "new\n")
        .write("src/b.ts", "untracked\n");
    let repo = open(&t);

    let rev = repo.at_rev("HEAD").unwrap();
    assert_eq!(
        rev.read(&paths(&["src/a.ts", "src/nope.ts", "src", "src/crlf.ts"]))
            .unwrap(),
        [
            Some(b"old\n".to_vec()),
            None,
            None,
            Some(b"a\r\nb\r\n".to_vec()),
        ]
    );
    assert_eq!(
        repo.worktree()
            .read(&paths(&["src/a.ts", "src/b.ts", "src/nope.ts"]))
            .unwrap(),
        [Some(b"new\n".to_vec()), Some(b"untracked\n".to_vec()), None]
    );
}

#[test]
fn read_of_a_revision_answers_a_long_list_in_order() {
    let t = TempRepo::new();
    t.write("src/a.ts", "a\n").commit("init");
    // More than a pipe buffer each way: it hangs unless requests and answers flow together.
    let mut many: Vec<Utf8PathBuf> = (0..2000)
        .map(|i| Utf8PathBuf::from(format!("src/missing-{i}.ts")))
        .collect();
    many.insert(1000, "src/a.ts".into());

    let read = open(&t).at_rev("HEAD").unwrap().read(&many).unwrap();

    assert_eq!(read.len(), 2001);
    assert_eq!(read[1000], Some(b"a\n".to_vec()));
    assert_eq!(read.iter().filter(|file| file.is_some()).count(), 1);
}

#[test]
fn head_and_files() {
    let t = TempRepo::new();
    t.write(".gitignore", "ignored.ts\n")
        .write("src/a.ts", "a\n")
        .commit("one");
    let one = t.git(&["rev-parse", "HEAD"]);
    let repo = open(&t);
    let rev = repo.at_rev("HEAD").unwrap();
    t.write("b.ts", "b\n").commit("two");
    t.write("c.ts", "c\n").write("ignored.ts", "x\n");

    assert_eq!(
        repo.worktree().head().unwrap().to_string(),
        t.git(&["rev-parse", "HEAD"])
    );
    assert_eq!(rev.head().unwrap().to_string(), one);
    assert_eq!(
        repo.worktree().files().unwrap(),
        paths(&[".gitignore", "b.ts", "c.ts", "src/a.ts"])
    );
    assert_eq!(rev.files().unwrap(), paths(&[".gitignore", "src/a.ts"]));
}

#[test]
fn files_of_the_worktree_lists_a_conflicted_file_once() {
    let t = TempRepo::new();
    t.write("a.ts", "base\n").commit("base");
    let base = t.git(&["rev-parse", "HEAD"]);
    t.branch("feat").write("a.ts", "feat\n").commit("feat");
    t.checkout("main").write("a.ts", "main\n").commit("main");
    // Leaves `a.ts` in the index at stages 1, 2 and 3, as a merge with a conflict does.
    t.git(&["read-tree", "-m", &base, "main", "feat"]);

    assert_eq!(open(&t).worktree().files().unwrap(), paths(&["a.ts"]));
}

#[test]
fn grep_word_finds_the_whole_word() {
    let t = TempRepo::new();
    t.write("a.ts", "enum Color {}\n")
        .write("b.ts", "items.enumerate();\n")
        .write("c.ts", "const my_enum = 1;\n")
        .commit("init");
    t.write("a.ts", "const x = 1;\n")
        .write("d.ts", "export enum Size {}\n");
    let repo = open(&t);

    assert_eq!(repo.worktree().grep_word("enum").unwrap(), paths(&["d.ts"]));
    assert_eq!(
        repo.at_rev("HEAD").unwrap().grep_word("enum").unwrap(),
        paths(&["a.ts"])
    );
    assert_eq!(repo.worktree().grep_word("absent").unwrap(), paths(&[]));
}

#[test]
fn write_replaces_the_file_and_keeps_its_permissions() {
    let t = TempRepo::new();
    t.write("src/a.ts", "old\n").commit("init");
    let file = t.path().join("src/a.ts");
    #[cfg(unix)]
    fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    let Snapshot::WorkTree(tree) = open(&t).worktree() else {
        panic!("worktree() is a WorkTree");
    };

    tree.write(Utf8Path::new("src/a.ts"), b"new\n").unwrap();

    assert_eq!(fs::read(&file).unwrap(), b"new\n");
    let names: Vec<_> = fs::read_dir(t.path().join("src"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["a.ts"]);
    #[cfg(unix)]
    {
        let mode = fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    match tree.write(Utf8Path::new("src/nope.ts"), b"x\n") {
        Err(RepoError::Io { source, .. }) => assert_eq!(source.kind(), ErrorKind::NotFound),
        other => panic!("{other:?}"),
    }
    assert!(!t.path().join("src/nope.ts").exists());
}

#[test]
fn a_git_error_carries_the_command_and_stderr() {
    let t = TempRepo::new();
    t.write("a.ts", "a\n").commit("init");

    match open(&t).at_rev("no-such") {
        Err(RepoError::Git { command, stderr }) => {
            assert!(command.contains("rev-parse"), "{command}");
            assert!(command.contains("no-such"), "{command}");
            assert!(!stderr.is_empty());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn repo_and_snapshot_can_be_shared_between_threads() {
    fn assert_sync<T: Send + Sync>() {}
    assert_sync::<Repo>();
    assert_sync::<Snapshot>();
}
