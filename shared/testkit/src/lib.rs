use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use tempfile::TempDir;

/// The variables `gitcmd` clears too. Set when tests run from a git hook or by the shell, they
/// point git at the outer repo or change how the `*.ts` pathspecs of the tests match.
const CLEARED_ENV: [&str; 11] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_LITERAL_PATHSPECS",
    "GIT_GLOB_PATHSPECS",
    "GIT_NOGLOB_PATHSPECS",
    "GIT_ICASE_PATHSPECS",
    "GIT_DIFF_OPTS",
];

/// A git repo in a temporary folder, removed on drop.
pub struct TempRepo {
    dir: TempDir,
}

impl TempRepo {
    #[expect(
        clippy::new_without_default,
        reason = "a `Default` that runs `git init` would be a surprise"
    )]
    pub fn new() -> TempRepo {
        let repo = TempRepo {
            dir: tempfile::tempdir().expect("create a temp folder"),
        };
        repo.git(&["init", "-q", "-b", "main"]);
        // The local config beats global and system ones, also for the calls of `gitcmd` in
        // tests: CI runners have no identity, Windows sets `core.autocrlf=true`, a global
        // fsmonitor holds the temp folder, global hooks still run on checkout and after a
        // commit (`--no-verify` skips only `pre-commit` and `commit-msg`), and global
        // attributes and ignores change what tests write. `hooksPath` names a folder that
        // does not exist. One append instead of a `git config` per key: a git start costs tens
        // of ms on Windows.
        let config = repo.path().join(".git/config");
        fs::OpenOptions::new()
            .append(true)
            .open(&config)
            .and_then(|mut file| {
                file.write_all(
                    b"[user]\n\tname = test\n\temail = test@stop-slop.invalid\n\
                      [core]\n\tautocrlf = false\n\tfsmonitor = false\n\
                      \thooksPath = .git/no-hooks\n\tattributesFile =\n\texcludesFile =\n\
                      [commit]\n\tgpgsign = false\n\
                      [tag]\n\tgpgsign = false\n",
                )
            })
            .unwrap_or_else(|error| panic!("{}: {error}", config.display()));
        repo
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Writes `text` as is and creates the folders on the way.
    pub fn write(&self, path: &str, text: &str) -> &Self {
        let full = self.path().join(path);
        let dir = full.parent().expect("a file in the repo has a folder");
        fs::create_dir_all(dir).unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
        fs::write(&full, text).unwrap_or_else(|error| panic!("{}: {error}", full.display()));
        self
    }

    pub fn commit(&self, message: &str) -> &Self {
        self.git(&["add", "-A"]);
        // `--no-verify` skips the hooks of the global config.
        self.git(&["commit", "-q", "--no-verify", "-m", message]);
        self
    }

    pub fn branch(&self, name: &str) -> &Self {
        self.git(&["checkout", "-q", "-b", name]);
        self
    }

    pub fn checkout(&self, name: &str) -> &Self {
        self.git(&["checkout", "-q", name]);
        self
    }

    /// Returns stdout without the trailing newline; panics with the command and stderr
    /// when git fails.
    pub fn git(&self, args: &[&str]) -> String {
        let mut command = Command::new("git");
        for name in CLEARED_ENV {
            command.env_remove(name);
        }
        let output = command
            .args(args)
            .current_dir(self.path())
            .stdin(Stdio::null())
            .output()
            .expect("start git");
        assert!(
            output.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_owned()
    }
}
