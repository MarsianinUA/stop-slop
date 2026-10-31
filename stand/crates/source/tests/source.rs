use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use camino::{Utf8Path, Utf8PathBuf};
use gitcmd::Git;
use source::{Before, SourceError, Tree, Worktree};
use tar::EntryType;
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

/// What no call may change: the index, `git status`, the refs but the one `pin` writes and
/// `forget` removes, and every file of the work tree.
#[derive(Debug, PartialEq)]
struct State {
    index: Vec<u8>,
    status: String,
    refs: String,
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
        refs: t.git(&["for-each-ref", "--format=%(refname) %(objectname)"]),
        files: walk(t.path())
            .into_iter()
            .map(|(name, path)| (name, fs::read(path).unwrap()))
            .collect(),
    }
}

/// Files and links under `root` but `.git`, by their `/`-separated paths; links are not followed.
fn walk(root: &Path) -> BTreeMap<String, PathBuf> {
    let mut found = BTreeMap::new();
    let mut folders = vec![root.to_owned()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                if entry.file_name() != ".git" {
                    folders.push(path);
                }
            } else {
                let name = path.strip_prefix(root).unwrap().to_string_lossy();
                found.insert(name.replace('\\', "/"), path);
            }
        }
    }
    found
}

fn archive(worktree: &Worktree, tree: &Tree) -> Vec<u8> {
    let mut tar = Vec::new();
    source::archive(worktree, tree, &mut tar).unwrap();
    tar
}

#[derive(Debug, PartialEq)]
enum Unpacked {
    File { mode: u32, bytes: Vec<u8> },
    Link(Vec<u8>),
}

/// The entries of a tar by path, read with the `tar` crate. Anything but files and links fails.
fn unpack(tar: &[u8]) -> BTreeMap<String, Unpacked> {
    let mut archive = tar::Archive::new(tar);
    let mut unpacked = BTreeMap::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = String::from_utf8(entry.path_bytes().into_owned()).unwrap();
        let mode = entry.header().mode().unwrap();
        let value = match entry.header().entry_type() {
            EntryType::Regular => {
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                Unpacked::File { mode, bytes }
            }
            EntryType::Symlink => Unpacked::Link(entry.link_name_bytes().unwrap().into_owned()),
            other => panic!("{path}: {other:?}"),
        };
        unpacked.insert(path, value);
    }
    unpacked
}

fn file(mode: u32, text: &str) -> Unpacked {
    Unpacked::File {
        mode,
        bytes: text.as_bytes().to_vec(),
    }
}

/// Path to the mode and oid of its blob, as `ls-tree` lists a tree.
type Listing = BTreeMap<String, (String, String)>;

fn listed(t: &TempRepo, tree: &Tree) -> Listing {
    let out = t.git(&["ls-tree", "-r", "-z", &tree.to_string()]);
    out.split('\0')
        .filter(|record| !record.is_empty())
        .map(|record| {
            let (fields, path) = record.split_once('\t').unwrap();
            let fields: Vec<&str> = fields.split(' ').collect();
            (
                path.to_owned(),
                (fields[0].to_owned(), fields[2].to_owned()),
            )
        })
        .collect()
}

/// The entries of a tar listed as git would list them: `hash-object --stdin` hashes the bytes as
/// they are, without filters.
fn as_listed(t: &TempRepo, unpacked: &BTreeMap<String, Unpacked>) -> Listing {
    unpacked
        .iter()
        .map(|(path, value)| {
            let (mode, bytes) = match value {
                Unpacked::File { mode: 0o755, bytes } => ("100755", bytes),
                Unpacked::File { mode: 0o644, bytes } => ("100644", bytes),
                Unpacked::File { mode, .. } => panic!("{path}: mode {mode:o}"),
                Unpacked::Link(target) => ("120000", target),
            };
            (path.clone(), (mode.to_owned(), hash(t, bytes)))
        })
        .collect()
}

fn hash(t: &TempRepo, bytes: &[u8]) -> String {
    let out = Git::at(t.path())
        .run_with_input(&["hash-object", "--stdin"], bytes)
        .unwrap();
    String::from_utf8(out).unwrap().trim_end().to_owned()
}

/// On unix a real link. On Windows the repo has `core.symlinks=false` and the link is a file
/// holding its target, as a checkout without the privilege for links leaves it; git still keeps
/// it as a link.
fn symlink(t: &TempRepo, target: &str, link: &str) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, t.path().join(link)).unwrap();
    #[cfg(windows)]
    {
        t.git(&["config", "core.symlinks", "false"]);
        t.write(link, target);
        let oid = t.git(&["hash-object", "-w", link]);
        let entry = format!("120000,{oid},{link}");
        t.git(&["update-index", "--add", "--cacheinfo", &entry]);
    }
}

/// On Windows `core.fileMode` is false: the bit lives only in the index.
fn executable(t: &TempRepo, path: &str) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let full = t.path().join(path);
        fs::set_permissions(full, fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(windows)]
    t.git(&["add", "--chmod=+x", path]);
}

/// What the system `tar` extracts, listed as git would list it: an extractor other than the
/// writer's own crate.
#[cfg(unix)]
fn extracted(t: &TempRepo, tar: &[u8]) -> Listing {
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new("tar")
        .args(["-x", "-f", "-", "-C"])
        .arg(dir.path())
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(tar).unwrap();
    assert!(child.wait().unwrap().success());
    walk(dir.path())
        .into_iter()
        .map(|(name, path)| {
            let meta = fs::symlink_metadata(&path).unwrap();
            let (mode, bytes) = if meta.file_type().is_symlink() {
                let target = fs::read_link(&path).unwrap();
                ("120000", target.as_os_str().as_bytes().to_vec())
            } else if meta.permissions().mode() & 0o111 != 0 {
                ("100755", fs::read(&path).unwrap())
            } else {
                ("100644", fs::read(&path).unwrap())
            };
            (name, (mode.to_owned(), hash(t, &bytes)))
        })
        .collect()
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
    let tar = archive(&worktree, &tree);
    source::pin(&worktree, &tree, at(1_800_000_000)).unwrap();
    source::before(&worktree, &Before::Session).unwrap();
    let committed = source::before(&worktree, &Before::Ref("main".to_owned())).unwrap();
    source::changed(&worktree, &committed.tree, &tree).unwrap();
    source::forget(&worktree).unwrap();

    assert_eq!(state(&t), before);
    let unpacked = unpack(&tar);
    let expected = BTreeMap::from([
        (".gitignore".to_owned(), file(0o644, "*.log\n")),
        ("edited.txt".to_owned(), file(0o644, "two\n")),
        ("new/untracked.txt".to_owned(), file(0o644, "new\n")),
        ("staged.txt".to_owned(), file(0o644, "on disk\n")),
    ]);
    assert_eq!(unpacked, expected);
}

#[test]
fn a_snapshot_takes_a_file_edited_in_the_second_its_index_was_written() {
    let t = TempRepo::new();
    // Only the size and the mtime in seconds tell git that a file changed, on every OS.
    t.git(&["config", "core.checkStat", "minimal"]);
    let date = |path: &str| {
        let file = fs::File::options()
            .write(true)
            .open(t.path().join(path))
            .unwrap();
        file.set_modified(at(1_600_000_000)).unwrap();
    };
    t.write("a.txt", "one\n");
    date("a.txt");
    t.git(&["add", "a.txt"]);
    t.write("a.txt", "two\n");
    date("a.txt");
    // As if git wrote the index in that second too: the entry may be stale.
    date(".git/index");

    let tree = source::snapshot(&open(&t)).unwrap();
    assert_eq!(
        t.git(&["cat-file", "blob", &format!("{tree}:a.txt")]),
        "two"
    );
}

#[test]
fn archive_keeps_what_git_archive_drops_and_unpacks_to_the_blobs_of_the_tree() {
    let t = TempRepo::new();
    let long = format!("{}/file.txt", ["a-folder-with-a-long-name"; 4].join("/"));
    t.write(
        ".gitattributes",
        "skip.txt export-ignore\nsubst.txt export-subst\n",
    )
    .write("skip.txt", "kept\n")
    .write("subst.txt", "$Format:%H$\n")
    .write("crlf.txt", "one\r\ntwo\r\n")
    .write("run.sh", "#!/bin/sh\necho run\n")
    .write("target.txt", "target\n")
    .write(&long, "deep\n");
    symlink(&t, "target.txt", "link");
    symlink(&t, &long, "long-link");
    executable(&t, "run.sh");
    t.commit("first");
    let worktree = open(&t);
    let tree = source::snapshot(&worktree).unwrap();
    assert_eq!(tree.to_string(), t.git(&["rev-parse", "HEAD^{tree}"]));

    let tar = archive(&worktree, &tree);
    let unpacked = unpack(&tar);
    assert_eq!(as_listed(&t, &unpacked), listed(&t, &tree));
    assert_eq!(unpacked["skip.txt"], file(0o644, "kept\n"));
    assert_eq!(unpacked["subst.txt"], file(0o644, "$Format:%H$\n"));
    assert_eq!(unpacked["crlf.txt"], file(0o644, "one\r\ntwo\r\n"));
    assert_eq!(unpacked["run.sh"], file(0o755, "#!/bin/sh\necho run\n"));
    assert_eq!(unpacked["link"], Unpacked::Link(b"target.txt".to_vec()));
    assert_eq!(
        unpacked["long-link"],
        Unpacked::Link(long.as_bytes().to_vec())
    );
    assert_eq!(unpacked[&long], file(0o644, "deep\n"));
    #[cfg(unix)]
    assert_eq!(extracted(&t, &tar), listed(&t, &tree));
}

#[test]
fn archive_writes_the_bytes_git_stores_not_the_line_ends_on_disk() {
    let t = TempRepo::new();
    t.git(&["config", "core.autocrlf", "true"]);
    t.write("text.txt", "one\r\ntwo\r\n").commit("first");
    let worktree = open(&t);
    let tree = source::snapshot(&worktree).unwrap();
    let unpacked = unpack(&archive(&worktree, &tree));
    assert_eq!(unpacked["text.txt"], file(0o644, "one\ntwo\n"));
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
    t.git(&["checkout", "-q", "--orphan", "unborn"]);
    let unborn = source::before(&worktree, &Before::Ref("main".to_owned()));
    assert!(
        matches!(&unborn, Err(SourceError::NoMergeBase(name)) if name == "main"),
        "{unborn:?}"
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
