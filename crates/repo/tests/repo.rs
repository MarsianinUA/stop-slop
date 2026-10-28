use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use camino::{Utf8Path, Utf8PathBuf};
use repo::{
    BranchDistance, BranchTip, Change, ChangedFile, LineRange, Oid, Repo, RepoError, Snapshot,
};
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

/// `ahead` and `behind` as `rev-list` counts them, with a TAB between.
fn distance(d: &BranchDistance) -> String {
    format!("{}\t{}", d.ahead, d.behind)
}

#[test]
fn branch_distances_count_the_commits_on_each_side_of_the_head() {
    let t = TempRepo::new();
    t.write("m.ts", "m\n").commit("init");
    t.branch("a").write("a.ts", "a\n").commit("a");
    t.branch("b").write("b.ts", "b\n").commit("b");
    t.branch("c").write("c.ts", "c\n").commit("c");
    t.git(&["update-ref", "refs/remotes/origin/c", "c"]);
    t.write("c2.ts", "c2\n").commit("c2 not pushed");
    t.branch("d").write("d.ts", "d\n").commit("d");
    t.checkout("b").write("b2.ts", "b2\n").commit("b moves on");
    t.checkout("main")
        .write("m2.ts", "m2\n")
        .commit("main moves on");
    set_origin_main(&t, &t.git(&["rev-parse", "main"]));
    t.git(&["update-ref", "refs/remotes/upstream/x", "main"]);
    t.git(&["tag", "v1"]);
    t.git(&["checkout", "-q", "--orphan", "lone"]);
    t.commit("lone");
    t.checkout("c");
    let repo = open(&t);

    let distances = repo.worktree().branch_distances().unwrap();

    let tips: Vec<BranchTip> = distances.iter().map(|d| d.tip.clone()).collect();
    assert_eq!(tips, repo.branches().unwrap());
    for d in &distances {
        let range = format!("{}...HEAD", d.tip.name);
        let counts = t.git(&["rev-list", "--left-right", "--count", &range]);
        assert_eq!(distance(d), counts, "{}", d.tip.name);
    }
    let table: Vec<(&str, u32, u32)> = distances
        .iter()
        .map(|d| (d.tip.name.as_str(), d.ahead, d.behind))
        .collect();
    assert_eq!(
        table,
        [
            ("a", 0, 3),
            ("b", 1, 2),
            ("c", 0, 0),
            ("d", 1, 0),
            ("lone", 1, 5),
            ("main", 1, 4),
            ("origin/c", 0, 1),
            ("origin/main", 1, 4),
        ]
    );

    let b = t.git(&["rev-parse", "b"]);
    let at_b = repo.at_rev("b").unwrap().branch_distances().unwrap();
    assert_eq!(at_b.len(), distances.len());
    for d in &at_b {
        let range = format!("{}...{b}", d.tip.name);
        let counts = t.git(&["rev-list", "--left-right", "--count", &range]);
        assert_eq!(distance(d), counts, "{}", d.tip.name);
    }
    let of_b = at_b.iter().find(|d| d.tip.name == "b").expect("b");
    assert_eq!((of_b.ahead, of_b.behind), (0, 0));
}

#[test]
fn branch_distances_leave_out_a_ref_to_a_tree() {
    let t = TempRepo::new();
    t.write("a.ts", "a\n").commit("init");
    let tree = t.git(&["rev-parse", "HEAD^{tree}"]);
    t.git(&["update-ref", "refs/remotes/origin/tree", &tree]);

    let distances = open(&t).worktree().branch_distances().unwrap();

    let names: Vec<&str> = distances.iter().map(|d| d.tip.name.as_str()).collect();
    assert_eq!(names, ["main"]);
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
fn read_of_a_revision_keeps_the_order_after_a_path_with_a_newline() {
    let t = TempRepo::new();
    t.write("a.ts", "a\n").write("b.ts", "b\n").commit("init");

    let read = open(&t)
        .at_rev("HEAD")
        .unwrap()
        .read(&paths(&["a.ts", "x\nb.ts", "b.ts"]))
        .unwrap();

    assert_eq!(read, [Some(b"a\n".to_vec()), None, Some(b"b\n".to_vec())]);
}

#[test]
fn read_of_a_revision_follows_a_symlink() {
    let t = TempRepo::new();
    t.write("real/base.json", "{}\n").commit("init");
    // The links go straight into the index: a symlink on disk needs rights on Windows.
    let link = |path: &str, target: &str| {
        t.write("target.txt", target);
        let blob = t.git(&["hash-object", "-w", "target.txt"]);
        let entry = format!("120000,{blob},{path}");
        t.git(&["update-index", "--add", "--cacheinfo", &entry]);
    };
    link("link.json", "real/base.json");
    link("dir", "real");
    link("dangling.json", "missing.json");
    link("loop.json", "loop.json");
    link("out.json", "../out.json");
    t.git(&["commit", "-q", "--no-verify", "-m", "links"]);

    let read = open(&t)
        .at_rev("HEAD")
        .unwrap()
        .read(&paths(&[
            "link.json",
            "dir/base.json",
            "dangling.json",
            "loop.json",
            "out.json",
            "real/base.json",
        ]))
        .unwrap();

    let json = Some(b"{}\n".to_vec());
    assert_eq!(read, [json.clone(), json.clone(), None, None, None, json]);
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
fn write_leaves_a_read_only_file_alone() {
    let t = TempRepo::new();
    t.write("src/a.ts", "old\n").commit("init");
    let file = t.path().join("src/a.ts");
    let writable = fs::metadata(&file).unwrap().permissions();
    let mut read_only = writable.clone();
    read_only.set_readonly(true);
    fs::set_permissions(&file, read_only).unwrap();
    let Snapshot::WorkTree(tree) = open(&t).worktree() else {
        panic!("worktree() is a WorkTree");
    };

    let written = tree.write(Utf8Path::new("src/a.ts"), b"new\n");

    let text = fs::read(&file).unwrap();
    let names: Vec<_> = fs::read_dir(t.path().join("src"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    // Windows cannot remove the temp folder with a read-only file in it.
    fs::set_permissions(&file, writable).unwrap();
    match written {
        Err(RepoError::Io { source, .. }) => {
            assert_eq!(source.kind(), ErrorKind::PermissionDenied);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(text, b"old\n");
    assert_eq!(names, ["a.ts"]);
}

#[test]
fn a_git_error_carries_the_command_and_stderr() {
    let t = TempRepo::new();

    // Without a commit, `HEAD` names none.
    match open(&t).worktree().head() {
        Err(RepoError::Git { command, stderr }) => {
            assert!(command.contains("rev-parse"), "{command}");
            assert!(command.contains("HEAD"), "{command}");
            assert!(!stderr.is_empty());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn at_rev_of_a_name_that_is_not_a_commit_is_unknown_rev() {
    let t = TempRepo::new();
    t.write("a.ts", "a\n").commit("init");
    let tree = t.git(&["rev-parse", "HEAD^{tree}"]);

    for rev in ["no-such", "-x", tree.as_str()] {
        match open(&t).at_rev(rev) {
            Err(RepoError::UnknownRev(name)) => assert_eq!(name, rev),
            other => panic!("{rev}: {other:?}"),
        }
    }
}

#[test]
fn fetch_pr_takes_the_head_and_the_base_from_origin() {
    let t = TempRepo::new();
    t.write("a.ts", "a\n").commit("main");
    t.branch("pr").write("a.ts", "pr\n").commit("pr");
    let (main, head) = (t.git(&["rev-parse", "main"]), t.git(&["rev-parse", "pr"]));
    let origin = tempfile::tempdir().unwrap();
    let url = origin.path().to_str().expect("a UTF-8 temp path");
    t.git(&["init", "-q", "--bare", url]);
    // By URL: no `origin/*` refs exist before the fetch.
    t.git(&[
        "push",
        "-q",
        url,
        "main",
        "pr:refs/pull/1/head",
        "pr:refs/tags/v1",
    ]);
    t.git(&["remote", "add", "origin", url]);

    open(&t).fetch_pr(1, "main", &head).unwrap();
    assert_eq!(t.git(&["rev-parse", "refs/stop-slop/pr-1"]), head);
    assert_eq!(t.git(&["rev-parse", "refs/remotes/origin/main"]), main);
    // A plain fetch would bring `v1` along, as it sits on the head.
    assert_eq!(t.git(&["tag", "--list"]), "");

    match open(&t).fetch_pr(1, "main", &main) {
        Err(RepoError::PrMoved { number: 1 }) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn repo_and_snapshot_can_be_shared_between_threads() {
    fn assert_sync<T: Send + Sync>() {}
    assert_sync::<Repo>();
    assert_sync::<Snapshot>();
}

const GLOBS: &[&str] = &["*.ts", "*.tsx", "*.mts", "*.cts"];

fn changed(path: &str, change: Change, ranges: &[(u32, u32)]) -> ChangedFile {
    ChangedFile {
        path: path.into(),
        change,
        lines: ranges
            .iter()
            .map(|&(start, end)| LineRange { start, end })
            .collect(),
    }
}

fn base(repo: &Repo) -> Oid {
    repo.resolve("main").unwrap().expect("main")
}

#[test]
fn changes_of_the_worktree_take_commits_edits_and_untracked_files() {
    let t = TempRepo::new();
    t.write("src/keep.ts", "a\nb\nc\nd\ne\nf\n")
        .write("src/gone.ts", "gone\n")
        .write("src/dirty.ts", "old\n")
        .write("src/dropped.ts", "dropped\n")
        .write("notes.md", "old\n")
        .commit("init");
    t.branch("feat")
        .write("src/keep.ts", "a\nB\nc\nD\ne\n")
        .commit("edit");
    t.write("src/new.ts", "1\n2\n3\n").commit("add");
    fs::remove_file(t.path().join("src/gone.ts")).unwrap();
    t.write("notes.md", "new\n").commit("remove");
    t.write("src/dirty.ts", "new\n")
        .write("src/staged.ts", "staged\n")
        .write("src/loose.ts", "loose\n")
        .write("x.js", "loose\n");
    t.git(&["add", "src/staged.ts"]);
    // `diff` calls it deleted, `ls-files --others` untracked; one entry stays.
    t.git(&["rm", "-q", "--cached", "src/dropped.ts"]);
    let repo = open(&t);

    assert_eq!(
        repo.worktree().changes(&base(&repo), GLOBS).unwrap(),
        [
            changed("src/dirty.ts", Change::Modified, &[(1, 1)]),
            changed("src/dropped.ts", Change::Untracked, &[(1, u32::MAX)]),
            changed("src/gone.ts", Change::Deleted, &[]),
            changed("src/keep.ts", Change::Modified, &[(2, 2), (4, 4)]),
            changed("src/loose.ts", Change::Untracked, &[(1, u32::MAX)]),
            changed("src/new.ts", Change::Added, &[(1, 3)]),
            changed("src/staged.ts", Change::Added, &[(1, 1)]),
        ]
    );
}

#[test]
fn changes_list_the_files_of_merge_base_and_diff() {
    let t = TempRepo::new();
    t.write("a.ts", "a\n")
        .write("b.ts", "b\n")
        .write("main.ts", "main\n")
        .commit("init");
    t.branch("feat").write("a.ts", "feat\n").commit("feat");
    t.checkout("main")
        .write("main.ts", "ahead\n")
        .commit("ahead");
    t.checkout("feat")
        .write("b.ts", "dirty\n")
        .write("c.ts", "new\n");
    let repo = open(&t);

    let found: BTreeSet<Utf8PathBuf> = repo
        .worktree()
        .changes(&base(&repo), GLOBS)
        .unwrap()
        .into_iter()
        .map(|file| file.path)
        .collect();

    let fork = t.git(&["merge-base", "main", "HEAD"]);
    let mut diff = vec!["diff", "--name-only", &fork, "--"];
    diff.extend(GLOBS);
    let mut others = vec!["ls-files", "--others", "--exclude-standard", "--"];
    others.extend(GLOBS);
    let expected: BTreeSet<Utf8PathBuf> = [t.git(&diff), t.git(&others)]
        .iter()
        .flat_map(|out| out.lines().map(Utf8PathBuf::from))
        .collect();
    assert_eq!(found, expected);
    assert_eq!(
        found,
        BTreeSet::from(["a.ts".into(), "b.ts".into(), "c.ts".into()])
    );
}

#[test]
fn changes_show_a_renamed_file_under_its_new_path_with_the_edit_only() {
    let t = TempRepo::new();
    t.write("src/old.ts", "1\n2\n3\n4\n5\n6\n7\n8\n")
        .commit("init");
    t.branch("feat");
    t.git(&["mv", "src/old.ts", "src/new.ts"]);
    t.write("src/new.ts", "1\n2\n3\nfour\n5\n6\n7\n8\n")
        .commit("rename");
    let repo = open(&t);

    assert_eq!(
        repo.worktree().changes(&base(&repo), GLOBS).unwrap(),
        [changed("src/new.ts", Change::Modified, &[(4, 4)])]
    );
}

#[test]
fn changes_read_odd_names_and_bodies() {
    let t = TempRepo::new();
    t.write("src/body.ts", "a\n-- gone\nc\n").commit("init");
    fs::write(t.path().join("src/bytes.ts"), b"a\nb\n").unwrap();
    t.commit("bytes");
    t.branch("feat")
        .write("src/body.ts", "a\nc\n++ x\n")
        .write("src/my file.ts", "x\n")
        .write("src/файл ё.ts", "x\n")
        .write("src/empty.ts", "");
    fs::write(t.path().join("src/bytes.ts"), b"a\n\xff\n").unwrap();
    t.commit("feat");
    let repo = open(&t);

    assert_eq!(
        repo.worktree().changes(&base(&repo), GLOBS).unwrap(),
        [
            changed("src/body.ts", Change::Modified, &[(3, 3)]),
            changed("src/bytes.ts", Change::Modified, &[(2, 2)]),
            changed("src/empty.ts", Change::Added, &[]),
            changed("src/my file.ts", Change::Added, &[(1, 1)]),
            changed("src/файл ё.ts", Change::Added, &[(1, 1)]),
        ]
    );
}

#[test]
fn changes_of_a_revision_skip_the_disk() {
    let t = TempRepo::new();
    t.write("a.ts", "a\n").write("b.ts", "b\n").commit("init");
    t.branch("feat").write("a.ts", "feat\n").commit("feat");
    t.write("b.ts", "dirty\n").write("c.ts", "untracked\n");
    let repo = open(&t);

    assert_eq!(
        repo.at_rev("feat")
            .unwrap()
            .changes(&base(&repo), GLOBS)
            .unwrap(),
        [changed("a.ts", Change::Modified, &[(1, 1)])]
    );
}

#[test]
fn differs_from_leaves_out_files_that_match_the_base() {
    let t = TempRepo::new();
    t.write("a.ts", "1\n")
        .write("b.ts", "1\n")
        .write("c.ts", "1\n")
        .commit("init");
    t.branch("feat")
        .write("a.ts", "2\n")
        .write("b.ts", "2\n")
        .commit("feat");
    // As a squash merge of another branch leaves it: `main` got the same `b.ts`.
    t.checkout("main").write("b.ts", "2\n").commit("squash");
    t.checkout("feat")
        .write("c.ts", "dirty\n")
        .write("d.ts", "untracked\n");
    let repo = open(&t);
    let main = base(&repo);

    assert_eq!(
        repo.worktree().differs_from(&main, GLOBS).unwrap(),
        BTreeSet::from(["a.ts".into(), "c.ts".into()])
    );
    assert_eq!(
        repo.at_rev("feat")
            .unwrap()
            .differs_from(&main, GLOBS)
            .unwrap(),
        BTreeSet::from(["a.ts".into()])
    );
}

#[test]
fn the_users_diff_config_changes_nothing() {
    let t = TempRepo::new();
    t.write("src/a.ts", "1\n2\n3\n4\n5\n")
        .write("src/old.ts", "1\n2\n3\n4\n5\n6\n7\n8\n")
        .commit("init");
    t.branch("feat")
        .write("src/a.ts", "1\ntwo\n3\nfour\n5\n")
        .write("src/e.ts", "enum E {}\n");
    t.git(&["mv", "src/old.ts", "src/new.ts"]);
    t.write("src/new.ts", "1\n2\n3\nfour\n5\n6\n7\n8\n")
        .commit("feat");
    let repo = open(&t);
    let main = base(&repo);
    let read = || {
        let tree = repo.worktree();
        (
            tree.changes(&main, GLOBS).unwrap(),
            tree.differs_from(&main, GLOBS).unwrap(),
            tree.grep_word("enum").unwrap(),
        )
    };
    let plain = read();
    assert_eq!(
        plain.0,
        [
            changed("src/a.ts", Change::Modified, &[(2, 2), (4, 4)]),
            changed("src/e.ts", Change::Added, &[(1, 1)]),
            changed("src/new.ts", Change::Modified, &[(4, 4)]),
        ]
    );

    for (key, value) in [
        ("diff.noprefix", "true"),
        ("diff.mnemonicPrefix", "true"),
        ("color.ui", "always"),
        ("diff.interHunkContext", "5"),
        ("diff.renames", "false"),
        ("diff.external", "no-such-diff-tool"),
        ("diff.algorithm", "patience"),
        ("diff.indentHeuristic", "false"),
        ("submodule.recurse", "true"),
    ] {
        t.git(&["config", key, value]);
    }

    assert_eq!(read(), plain);
}
