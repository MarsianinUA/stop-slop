use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use camino::{Utf8Path, Utf8PathBuf};
use gitcmd::Git;
use source::{Before, SourceError, Worktree};
use testkit::TempRepo;

fn utf8(path: &Path) -> &Utf8Path {
    Utf8Path::from_path(path).unwrap()
}

fn open(t: &TempRepo) -> Worktree {
    source::open(utf8(t.path())).unwrap()
}

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// What no call may change: the index, `git status` and every file of the work tree.
#[derive(Debug, PartialEq)]
struct State {
    index: Vec<u8>,
    status: String,
    files: BTreeMap<String, Vec<u8>>,
}

fn state(t: &TempRepo) -> State {
    State {
        index: fs::read(t.path().join(".git/index")).unwrap(),
        // Without optional locks status does not refresh the index it would compare.
        status: t.git(&[
            "--no-optional-locks",
            "status",
            "--porcelain",
            "-z",
            "--ignored",
            "--untracked-files=all",
        ]),
        files: files(t.path()),
    }
}

fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut folders = vec![root.to_owned()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().unwrap() != ".git" {
                    folders.push(path);
                }
            } else {
                let name = path.strip_prefix(root).unwrap().to_string_lossy();
                files.insert(name.replace('\\', "/"), fs::read(&path).unwrap());
            }
        }
    }
    files
}

fn show(t: &TempRepo, tree: &source::Tree, path: &str) -> String {
    t.git(&["show", &format!("{tree}:{path}")])
}

#[test]
fn a_snapshot_takes_the_files_on_disk_and_no_call_changes_the_repo() {
    let t = TempRepo::new();
    t.write(".gitignore", "*.log\n")
        .write("edited.txt", "one\n")
        .write("staged.txt", "one\n")
        .commit("first");
    t.write("edited.txt", "two\n")
        .write("staged.txt", "staged\n")
        .write("new/untracked.txt", "new\n")
        .write("ignored.log", "noise\n");
    t.git(&["add", "staged.txt"]);
    t.write("staged.txt", "on disk\n");
    let before = state(&t);

    let worktree = open(&t);
    let tree = source::snapshot(&worktree).unwrap();
    source::pin(&worktree, &tree, at(1_800_000_000)).unwrap();
    source::before(&worktree, &Before::Session).unwrap();
    let committed = source::before(&worktree, &Before::Ref("main".to_owned())).unwrap();
    source::changed(&worktree, &committed.tree, &tree).unwrap();
    source::forget(&worktree).unwrap();

    assert_eq!(state(&t), before);
    assert_eq!(show(&t, &tree, "edited.txt"), "two");
    assert_eq!(show(&t, &tree, "staged.txt"), "on disk");
    assert_eq!(show(&t, &tree, "new/untracked.txt"), "new");
    let paths = t.git(&["ls-tree", "-r", "--name-only", &tree.to_string()]);
    assert_eq!(
        paths,
        ".gitignore\nedited.txt\nnew/untracked.txt\nstaged.txt"
    );
}

#[test]
fn pin_points_the_ref_of_the_worktree_at_a_commit_without_parents_dated_now() {
    let t = TempRepo::new();
    t.write("a.txt", "one\n").commit("first");
    let worktree = open(&t);
    let tree = source::snapshot(&worktree).unwrap();
    let reference = format!("refs/stop-slop/stand/before/{}", worktree.key);

    let first = source::pin(&worktree, &tree, at(1_800_000_000)).unwrap();
    assert_eq!(first.replaced, None);
    assert_eq!(t.git(&["rev-parse", &reference]), first.commit.to_string());
    let raw = t.git(&["cat-file", "commit", &reference]);
    assert!(raw.starts_with(&format!("tree {tree}\n")), "{raw}");
    assert!(!raw.contains("\nparent "), "{raw}");
    assert!(raw.contains(" 1800000000 +0000\ncommitter "), "{raw}");
    assert!(raw.contains(" 1800000000 +0000\n\n"), "{raw}");

    let second = source::pin(&worktree, &tree, at(1_800_000_060)).unwrap();
    assert_eq!(second.replaced, Some(first.commit));
    assert_eq!(t.git(&["rev-parse", &reference]), second.commit.to_string());

    source::forget(&worktree).unwrap();
    assert_eq!(t.git(&["for-each-ref", "refs/stop-slop/"]), "");
}

#[test]
fn before_session_needs_a_snapshot_and_gives_its_tree_and_time() {
    let t = TempRepo::new();
    t.write("a.txt", "one\n").commit("first");
    let worktree = open(&t);
    let no_snapshot = source::before(&worktree, &Before::Session);
    assert!(
        matches!(no_snapshot, Err(SourceError::NoSnapshot)),
        "{no_snapshot:?}"
    );

    t.write("a.txt", "edited\n");
    let tree = source::snapshot(&worktree).unwrap();
    let pinned = source::pin(&worktree, &tree, at(1_800_000_000)).unwrap();
    let resolved = source::before(&worktree, &Before::Session).unwrap();
    assert_eq!(resolved.intent, Before::Session);
    assert_eq!(resolved.treeish, pinned.commit);
    assert_eq!(resolved.tree, tree);
    assert_eq!(resolved.taken_at, at(1_800_000_000));
}

#[test]
fn before_ref_gives_the_merge_base_and_its_commit_date() {
    let t = TempRepo::new();
    t.write("a.txt", "base\n").git(&["add", "-A"]);
    let date = "@1700000000 +0000";
    Git::at(t.path())
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .run(&["commit", "-q", "--no-verify", "-m", "base"])
        .unwrap();
    let base = t.git(&["rev-parse", "HEAD"]);
    let base_tree = t.git(&["rev-parse", "HEAD^{tree}"]);
    t.branch("feature")
        .write("a.txt", "feature\n")
        .commit("feature");
    t.checkout("main").write("b.txt", "main\n").commit("main");
    t.git(&["checkout", "-q", "--orphan", "other"]);
    t.commit("unrelated");
    t.checkout("feature");
    let worktree = open(&t);

    let resolved = source::before(&worktree, &Before::Ref("main".to_owned())).unwrap();
    assert_eq!(resolved.intent, Before::Ref("main".to_owned()));
    assert_eq!(resolved.treeish.to_string(), base);
    assert_eq!(resolved.tree.to_string(), base_tree);
    assert_eq!(resolved.taken_at, at(1_700_000_000));

    let unknown = source::before(&worktree, &Before::Ref("no-such".to_owned()));
    assert!(
        matches!(&unknown, Err(SourceError::UnknownRef(name)) if name == "no-such"),
        "{unknown:?}"
    );
    let option = source::before(&worktree, &Before::Ref("--all".to_owned()));
    assert!(
        matches!(option, Err(SourceError::UnknownRef(_))),
        "{option:?}"
    );
    let unrelated = source::before(&worktree, &Before::Ref("other".to_owned()));
    assert!(
        matches!(&unrelated, Err(SourceError::NoMergeBase(name)) if name == "other"),
        "{unrelated:?}"
    );
}

#[test]
fn two_worktrees_of_one_repo_get_their_own_keys_and_snapshots() {
    let t = TempRepo::new();
    t.write("a.txt", "one\n")
        .write("sub/b.txt", "two\n")
        .commit("first");
    let outside = tempfile::tempdir().unwrap();
    let second = outside.path().join("second");
    t.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "second",
        second.to_str().unwrap(),
    ]);
    fs::write(second.join("only-here.txt"), "three\n").unwrap();

    let first = open(&t);
    let other = source::open(utf8(&second)).unwrap();
    assert_ne!(first.key, other.key);
    assert_eq!(first.common, other.common);
    let from_sub = source::open(utf8(&t.path().join("sub"))).unwrap();
    assert_eq!(from_sub.key, first.key);
    assert_eq!(from_sub.root, first.root);

    let tree = source::snapshot(&other).unwrap();
    let paths = t.git(&["ls-tree", "-r", "--name-only", &tree.to_string()]);
    assert_eq!(paths, "a.txt\nonly-here.txt\nsub/b.txt");
    source::forget(&first).unwrap();

    let plain = tempfile::tempdir().unwrap();
    let outside_git = source::open(utf8(plain.path()));
    assert!(
        matches!(outside_git, Err(SourceError::NotARepo)),
        "{outside_git:?}"
    );
}

#[test]
fn changed_names_the_edited_added_and_removed_paths() {
    let t = TempRepo::new();
    t.write("same.txt", "same\n")
        .write("edited.txt", "one\n")
        .write("removed.txt", "bye\n")
        .commit("first");
    let worktree = open(&t);
    let before = source::snapshot(&worktree).unwrap();
    t.write("edited.txt", "two\n")
        .write("dir/with space.txt", "new\n")
        .write("ünïcode.txt", "new\n");
    fs::remove_file(t.path().join("removed.txt")).unwrap();
    let head = source::snapshot(&worktree).unwrap();

    let changed = source::changed(&worktree, &before, &head).unwrap();
    let expected = [
        "dir/with space.txt",
        "edited.txt",
        "removed.txt",
        "ünïcode.txt",
    ];
    assert_eq!(changed, expected.map(Utf8PathBuf::from));
    assert!(source::changed(&worktree, &head, &head).unwrap().is_empty());
}
