use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use repo::{LineRange, Oid, Repo, RepoError};
use scope::{
    Base, BaseReason, PrBase, Scope, ScopeError, ScopeRequest, ScopedFile, SkipReason, Skipped,
    resolve,
};
use testkit::TempRepo;

/// Answers `name` and keeps what it was asked. A `RefCell` is enough: `scope` asks on the
/// calling thread.
struct Answer {
    name: Option<&'static str>,
    asked: RefCell<Vec<(PathBuf, String)>>,
}

impl Answer {
    fn new(name: Option<&'static str>) -> Answer {
        Answer {
            name,
            asked: RefCell::new(Vec::new()),
        }
    }
}

impl PrBase for Answer {
    fn base_of(&self, root: &Path, branch: &str) -> Option<String> {
        let asked = (root.to_owned(), branch.to_owned());
        self.asked.borrow_mut().push(asked);
        self.name.map(str::to_owned)
    }
}

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

fn set_origin(t: &TempRepo, name: &str) {
    t.git(&["update-ref", &format!("refs/remotes/origin/{name}"), name]);
}

fn paths(files: &[ScopedFile]) -> Vec<String> {
    files.iter().map(|file| file.path.to_string()).collect()
}

fn branch(base: Option<&str>) -> ScopeRequest {
    ScopeRequest::Branch {
        base: base.map(str::to_owned),
    }
}

/// `Branch` on the work tree.
fn on_branch(t: &TempRepo, pr: &Answer, base: Option<&str>) -> Scope {
    let repo = Repo::open(t.path()).unwrap();
    resolve(&repo, &repo.worktree(), pr, branch(base)).unwrap()
}

fn base(scope: &Scope) -> (BaseReason, &str) {
    let base = scope.base.as_ref().expect("a base");
    (base.reason, &base.reference)
}

fn oid(t: &TempRepo, rev: &str) -> Oid {
    let repo = Repo::open(t.path()).unwrap();
    repo.resolve(rev).unwrap().expect("a commit")
}

fn lines(ranges: &[(u32, u32)]) -> Option<Vec<LineRange>> {
    Some(
        ranges
            .iter()
            .map(|&(start, end)| LineRange { start, end })
            .collect(),
    )
}

#[test]
fn base_comes_from_the_flag_then_the_pull_request_then_the_ancestor() {
    let t = TempRepo::new();
    t.write("m.ts", "m\n").commit("main");
    t.branch("a").write("a.ts", "a\n").commit("a");
    t.branch("b").write("b.ts", "b\n").commit("b");
    set_origin(&t, "a");
    set_origin(&t, "b");

    let pr = Answer::new(Some("a"));
    let scope = on_branch(&t, &pr, Some("main"));
    assert_eq!(
        scope.base,
        Some(Base {
            reference: "main".to_owned(),
            oid: oid(&t, "main"),
            reason: BaseReason::Flag,
        })
    );
    assert_eq!(paths(&scope.files), ["a.ts", "b.ts"]);
    assert!(pr.asked.borrow().is_empty());

    let scope = on_branch(&t, &pr, None);
    assert_eq!(base(&scope), (BaseReason::Pr, "origin/a"));
    assert_eq!(paths(&scope.files), ["b.ts"]);
    let asked = pr.asked.borrow();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].1, "b");
    assert_eq!(canon(&asked[0].0), canon(t.path()));
    drop(asked);

    t.git(&["update-ref", "-d", "refs/remotes/origin/a"]);
    let scope = on_branch(&t, &Answer::new(Some("a")), None);
    assert_eq!(base(&scope), (BaseReason::Pr, "a"));

    for answer in [Some("gone"), None] {
        let scope = on_branch(&t, &Answer::new(answer), None);
        assert_eq!(base(&scope), (BaseReason::Ancestor, "a"), "{answer:?}");
    }

    // Not on `origin`, so no pull request to ask about.
    t.git(&["update-ref", "-d", "refs/remotes/origin/b"]);
    let pr = Answer::new(Some("main"));
    let scope = on_branch(&t, &pr, None);
    assert_eq!(base(&scope), (BaseReason::Ancestor, "a"));
    assert!(pr.asked.borrow().is_empty());
}

#[test]
fn in_a_stack_the_base_is_the_previous_branch() {
    let t = TempRepo::new();
    t.write("m.ts", "m\n").commit("main");
    t.branch("a").write("a.ts", "a\n").commit("a");
    t.branch("b").write("b.ts", "b\n").commit("b");
    t.branch("c").write("c.ts", "c\n").commit("c");
    // `origin/c` is behind `c` and nearer than any other branch.
    set_origin(&t, "c");
    t.write("c2.ts", "c2\n").commit("c2");
    t.branch("d").write("d.ts", "d\n").commit("d");
    t.checkout("c");
    let pr = Answer::new(None);

    let scope = on_branch(&t, &pr, None);
    assert_eq!(
        scope.base,
        Some(Base {
            reference: "b".to_owned(),
            oid: oid(&t, "b"),
            reason: BaseReason::Ancestor,
        })
    );
    assert_eq!(paths(&scope.files), ["c.ts", "c2.ts"]);

    t.checkout("b").write("b2.ts", "b2\n").commit("b2");
    t.checkout("main").write("m2.ts", "m2\n").commit("m2");
    t.checkout("c");
    let scope = on_branch(&t, &pr, None);
    assert_eq!(base(&scope), (BaseReason::Ancestor, "b"));
    assert_eq!(scope.base.as_ref().unwrap().oid, oid(&t, "b"));
    assert_eq!(paths(&scope.files), ["c.ts", "c2.ts"]);

    // A tie: the local branch comes before `origin/*`.
    set_origin(&t, "b");
    let scope = on_branch(&t, &pr, None);
    assert_eq!(base(&scope), (BaseReason::Ancestor, "b"));
    assert!(scope.skipped.is_empty());
}

#[test]
fn files_a_squash_merge_brought_into_the_base_are_skipped() {
    let t = TempRepo::new();
    t.write("shared.ts", "1\n2\n3\n")
        .write("m.ts", "m\n")
        .commit("main");
    t.branch("a")
        .write("a.ts", "a\n")
        .write("shared.ts", "one\n2\n3\n")
        .commit("a");
    t.branch("b")
        .write("b.ts", "b\n")
        .write("shared.ts", "one\n2\nthree\n")
        .commit("b");
    t.checkout("main");
    t.git(&["merge", "-q", "--squash", "a"]);
    t.commit("squash a");
    // GitHub deletes a merged branch.
    t.git(&["branch", "-q", "-D", "a"]);
    t.checkout("b");
    let skipped = [Skipped {
        path: "a.ts".into(),
        reason: SkipReason::AlreadyInBase,
    }];

    let scope = on_branch(&t, &Answer::new(None), None);
    assert_eq!(base(&scope), (BaseReason::Ancestor, "main"));
    assert_eq!(paths(&scope.files), ["b.ts", "shared.ts"]);
    assert_eq!(scope.skipped, skipped);

    let scope = on_branch(&t, &Answer::new(None), Some("main"));
    assert_eq!(base(&scope), (BaseReason::Flag, "main"));
    assert_eq!(paths(&scope.files), ["b.ts", "shared.ts"]);
    assert_eq!(scope.skipped, skipped);
}

#[test]
fn without_an_ancestor_the_base_is_origin_head() {
    let t = TempRepo::new();
    t.write("m.ts", "m\n").commit("main");
    set_origin(&t, "main");
    t.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    t.write("m.ts", "changed\n");

    let scope = on_branch(&t, &Answer::new(None), None);
    assert_eq!(base(&scope), (BaseReason::Default, "origin/main"));
    assert_eq!(
        scope.files,
        [ScopedFile {
            path: "m.ts".into(),
            changed: lines(&[(1, 1)]),
        }]
    );

    t.git(&["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"]);
    let repo = Repo::open(t.path()).unwrap();
    let error = resolve(&repo, &repo.worktree(), &Answer::new(None), branch(None)).unwrap_err();
    assert!(matches!(error, ScopeError::NoBase), "{error:?}");
}

#[test]
fn only_typescript_files_without_declarations_are_checked() {
    let t = TempRepo::new();
    t.write("old.ts", "old\n")
        .write("keep.js", "keep\n")
        .commit("main");
    t.branch("feat");
    for name in [
        "a.ts", "b.tsx", "c.mts", "d.cts", "e.d.ts", "f.d.mts", "g.d.cts", "h.js", "i.json",
    ] {
        t.write(name, "x\n");
    }
    t.git(&["rm", "-q", "old.ts"]);
    t.commit("feat");
    let checked = ["a.ts", "b.tsx", "c.mts", "d.cts"];
    let repo = Repo::open(t.path()).unwrap();
    let pr = Answer::new(None);

    let scope = on_branch(&t, &pr, None);
    assert_eq!(paths(&scope.files), checked);
    assert!(scope.skipped.is_empty());

    let scope = resolve(&repo, &repo.worktree(), &pr, ScopeRequest::All).unwrap();
    assert_eq!(paths(&scope.files), checked);

    let listed = ["e.d.ts", "h.js", "a.ts"].map(PathBuf::from).to_vec();
    let scope = resolve(&repo, &repo.worktree(), &pr, ScopeRequest::Files(listed)).unwrap();
    assert_eq!(paths(&scope.files), ["a.ts"]);
}

#[test]
fn files_and_all_give_repo_paths_without_a_base() {
    let t = TempRepo::new();
    t.write("src/a.ts", "a\n")
        .write("src/b.ts", "b\n")
        .commit("init");
    let pr = Answer::new(None);
    let at_root = Repo::open(t.path()).unwrap();
    let only_a = |scope: Scope| {
        assert_eq!(
            scope,
            Scope {
                base: None,
                files: vec![ScopedFile {
                    path: "src/a.ts".into(),
                    changed: None,
                }],
                skipped: Vec::new(),
            }
        );
    };
    let files = |repo: &Repo, listed: Vec<PathBuf>| {
        resolve(repo, &repo.worktree(), &pr, ScopeRequest::Files(listed))
    };

    let absolute = t.path().join("src/a.ts");
    let in_src = Repo::open(&t.path().join("src")).unwrap();
    only_a(files(&in_src, vec![absolute.clone(), "a.ts".into(), absolute]).unwrap());

    let links = tempfile::tempdir().unwrap();
    let alias = links.path().join("alias");
    if symlink_dir_or_skip(t.path(), &alias) {
        only_a(files(&at_root, vec![alias.join("src/a.ts")]).unwrap());
    }

    #[cfg(windows)]
    only_a(files(&at_root, vec![r"src\a.ts".into()]).unwrap());

    let scope = resolve(&at_root, &at_root.worktree(), &pr, ScopeRequest::All).unwrap();
    assert_eq!(scope.base, None);
    assert_eq!(paths(&scope.files), ["src/a.ts", "src/b.ts"]);
    assert!(scope.files.iter().all(|file| file.changed.is_none()));

    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("x.ts"), "x\n").unwrap();
    let error = files(&at_root, vec![outside.path().join("x.ts")]).unwrap_err();
    assert!(
        matches!(error, ScopeError::Repo(RepoError::OutsideRepo(_))),
        "{error:?}"
    );
}

#[test]
fn on_a_revision_the_changes_come_from_the_revision() {
    let t = TempRepo::new();
    t.write("a.ts", "1\n2\n3\n4\n5\n")
        .write("b.ts", "b\n")
        .commit("main");
    t.branch("feat")
        .write("a.ts", "1\ntwo\n3\n4\n5\n")
        .commit("feat");
    t.write("a.ts", "1\ntwo\n3\nfour\n5\n")
        .write("b.ts", "changed\n")
        .write("c.ts", "c\n");
    let repo = Repo::open(t.path()).unwrap();
    let pr = Answer::new(None);

    let at_feat = repo.at_rev("feat").unwrap();
    let scope = resolve(&repo, &at_feat, &pr, branch(Some("main"))).unwrap();
    assert_eq!(
        scope.files,
        [ScopedFile {
            path: "a.ts".into(),
            changed: lines(&[(2, 2)]),
        }]
    );

    let scope = resolve(&repo, &repo.worktree(), &pr, branch(Some("main"))).unwrap();
    assert_eq!(paths(&scope.files), ["a.ts", "b.ts", "c.ts"]);
    assert_eq!(scope.files[0].changed, lines(&[(2, 2), (4, 4)]));
}

#[test]
fn a_bad_base_and_a_revision_without_a_base_are_errors() {
    let t = TempRepo::new();
    t.write("a.ts", "a\n").commit("init");
    let repo = Repo::open(t.path()).unwrap();
    let pr = Answer::new(Some("main"));

    let error = resolve(&repo, &repo.worktree(), &pr, branch(Some("no-such"))).unwrap_err();
    assert!(
        matches!(&error, ScopeError::UnknownBase(name) if name == "no-such"),
        "{error:?}"
    );

    let at_head = repo.at_rev("HEAD").unwrap();
    let error = resolve(&repo, &at_head, &pr, branch(None)).unwrap_err();
    assert!(matches!(error, ScopeError::RevWithoutBase), "{error:?}");
    assert!(pr.asked.borrow().is_empty());
}
