use std::io;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use crate::{PullRequest, ScopeError};

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
        let output = view(root, branch, "baseRefName", ".baseRefName").ok()?;
        if !output.status.success() {
            return None;
        }
        base_name(&output.stdout)
    }
}

pub fn pull_request(root: &Path, number: u32) -> Result<PullRequest, ScopeError> {
    let error = |message: String| ScopeError::PullRequest { number, message };
    // One value per line, the title last: it is the only one that may hold any text.
    let output = view(
        root,
        &number.to_string(),
        "headRefOid,baseRefName,title",
        ".headRefOid, .baseRefName, .title",
    )
    .map_err(|source| error(format!("gh: {source}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(error(format!("gh: {}", stderr.trim())));
    }
    from_view(number, &output.stdout)
        .ok_or_else(|| error("unexpected output of `gh pr view`".to_owned()))
}

/// `--jq` prints the bare values, so no JSON parser is needed.
fn view(root: &Path, target: &str, fields: &str, filter: &str) -> io::Result<Output> {
    Command::new("gh")
        .args(["pr", "view", target, "--json", fields, "--jq", filter])
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
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

fn from_view(number: u32, stdout: &[u8]) -> Option<PullRequest> {
    let mut lines = str::from_utf8(stdout).ok()?.splitn(3, '\n');
    let head = lines.next()?.trim();
    let base = lines.next()?.trim();
    let title = lines.next()?.trim_end();
    let is_oid = !head.is_empty() && head.bytes().all(|byte| byte.is_ascii_hexdigit());
    (is_oid && !base.is_empty()).then(|| PullRequest {
        number,
        head: head.to_owned(),
        base: base.to_owned(),
        title: title.to_owned(),
    })
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

    const HEAD: &str = "9f2c4e1d0b7a6c5e4f3a2b1c0d9e8f7a6b5c4d3e";

    #[test]
    fn from_view_takes_the_head_the_base_and_the_rest_as_the_title() {
        let pr = |title: &str| PullRequest {
            number: 7,
            head: HEAD.to_owned(),
            base: "dev".to_owned(),
            title: title.to_owned(),
        };
        for (stdout, title) in [
            (format!("{HEAD}\ndev\nCache prices\n"), "Cache prices"),
            (format!("{HEAD}\r\ndev\r\nFix: cache\r\n"), "Fix: cache"),
            (format!("{HEAD}\ndev\nTwo\nlines\n"), "Two\nlines"),
        ] {
            assert_eq!(from_view(7, stdout.as_bytes()), Some(pr(title)), "{stdout}");
        }
    }

    #[test]
    fn from_view_is_none_for_empty_or_broken_output() {
        for stdout in [
            String::new(),
            "\n".to_owned(),
            format!("{HEAD}\n"),
            format!("{HEAD}\ndev"),
            format!("{HEAD}\n\nTitle\n"),
            "\ndev\nTitle\n".to_owned(),
            "main\ndev\nTitle\n".to_owned(),
        ] {
            assert_eq!(from_view(7, stdout.as_bytes()), None, "{stdout:?}");
        }
        assert_eq!(from_view(7, b"\xff\ndev\nTitle\n"), None);
    }
}
