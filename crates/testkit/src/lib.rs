use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use tempfile::TempDir;

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
        // The local config beats global and system ones, also for the calls of `repo` in
        // tests: CI runners have no identity, and Windows sets `core.autocrlf=true`. One
        // append instead of a `git config` per key: a git start costs tens of ms on Windows.
        let config = repo.path().join(".git/config");
        fs::OpenOptions::new()
            .append(true)
            .open(&config)
            .and_then(|mut file| {
                file.write_all(
                    b"[user]\n\tname = test\n\temail = test@stop-slop.invalid\n\
                      [core]\n\tautocrlf = false\n\
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
        let output = Command::new("git")
            .args(args)
            .current_dir(self.path())
            // Set when tests run from a git hook: they would point git at the outer repo.
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
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
