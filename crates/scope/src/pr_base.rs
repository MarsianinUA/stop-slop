use std::path::Path;
use std::process::{Command, Stdio};

/// Where the base of a pull request comes from: GitHub in the program, a set answer in tests.
pub trait PrBase {
    /// The name of the base branch of the pull request of `branch`; `None` when there is no
    /// pull request, no `gh` or no network.
    fn base_of(&self, root: &Path, branch: &str) -> Option<String>;
}

/// Runs `gh pr view` in the repo root. Any failure is `None`: the search goes on without it.
#[derive(Debug)]
pub struct GhPrBase;

impl PrBase for GhPrBase {
    fn base_of(&self, root: &Path, branch: &str) -> Option<String> {
        if reads_as_number(branch) {
            return None;
        }
        let output = Command::new("gh")
            .args(["pr", "view", branch])
            // `--jq` prints the bare name, so no JSON parser is needed.
            .args(["--json", "baseRefName", "--jq", ".baseRefName"])
            .current_dir(root)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        base_name(&output.stdout)
    }
}

/// `gh pr view` takes `123` and `#123` for the number of a pull request, not for a branch.
fn reads_as_number(branch: &str) -> bool {
    let digits = branch.strip_prefix('#').unwrap_or(branch);
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}

fn base_name(stdout: &[u8]) -> Option<String> {
    let name = str::from_utf8(stdout).ok()?.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_of_digits_reads_as_a_pull_request_number() {
        assert!(reads_as_number("123"));
        assert!(reads_as_number("#123"));
        for branch in ["k-05", "fix/1", "1a", "#", ""] {
            assert!(!reads_as_number(branch), "{branch}");
        }
    }

    #[test]
    fn base_name_takes_the_trimmed_line() {
        assert_eq!(base_name(b"k-02\n").as_deref(), Some("k-02"));
        assert_eq!(base_name(b"k-02\r\n").as_deref(), Some("k-02"));
    }

    #[test]
    fn base_name_is_none_for_empty_or_not_utf8_output() {
        assert_eq!(base_name(b""), None);
        assert_eq!(base_name(b"\n"), None);
        assert_eq!(base_name(b"\xff\xfe"), None);
    }
}
