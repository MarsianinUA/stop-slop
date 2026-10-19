use std::fmt::Display;
use std::io::{self, Read as _};
use std::path::{self, Path, PathBuf};

use engine::{Finding, Mode};
use kit::Ruleset;
use kit::serde::Deserialize;
use repo::{Repo, RepoError};
use scope::{ScopeError, ScopeRequest};
use state::State;

use crate::check;
use crate::error::{CliError, Exit};
use crate::output;

/// Without `deny_unknown_fields`: Claude Code sends more fields.
#[derive(Deserialize)]
#[serde(crate = "kit::serde")]
struct Input {
    tool_input: ToolInput,
}

#[derive(Deserialize)]
#[serde(crate = "kit::serde")]
struct ToolInput {
    file_path: PathBuf,
}

/// Exit code 2 for findings too: Claude Code passes stderr to Claude only with code 2.
pub(crate) fn run() -> Result<Exit, CliError> {
    // A `Write` input carries the whole file; `from_slice` is faster than `from_reader`.
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .read_to_end(&mut bytes)
        .map_err(input_error)?;
    let input: Input = serde_json::from_slice(&bytes).map_err(input_error)?;
    // Not `canonicalize`: the filter of `errors` needs no file system, and `relativize`
    // resolves symlinks of the files that pass it. A relative path is from the cwd, the
    // project folder where Claude Code starts the hook.
    let file = path::absolute(&input.tool_input.file_path).map_err(input_error)?;
    let findings = errors(&rules::RULESET, &file)?;
    if findings.is_empty() {
        return Ok(Exit::Clean);
    }
    output::print_err(&output::hook_text(&findings));
    Ok(Exit::Failure)
}

fn input_error(error: impl Display) -> CliError {
    CliError::HookInput(error.to_string())
}

fn errors(set: &'static Ruleset, file: &Path) -> Result<Vec<Finding>, CliError> {
    // Before git: an edit of a `.md`, such as a review draft inside `.git/`, where git fails,
    // does not depend on the repo.
    if !scope::is_checked(file) {
        return Ok(Vec::new());
    }
    let dir = file
        .parent()
        .ok_or_else(|| CliError::HookInput(format!("`{}` has no folder", file.display())))?;
    // The repo of the file, not of the cwd: Claude may edit a file of another repo. A git that
    // refuses the folder (inside `.git/`, dubious ownership) leaves the marker unread, so it
    // counts as no marker; `enable` could not have run there either. A git that does not
    // start is a failure.
    let repo = match Repo::open(dir) {
        Err(RepoError::NotARepo | RepoError::Git { .. }) => return Ok(Vec::new()),
        repo => repo?,
    };
    let state = State::open(repo.git_common_dir());
    // Before `config.toml`: without the marker a broken one does not get in the way.
    if !state.is_enabled() {
        return Ok(Vec::new());
    }
    let engine = crate::engine_for(set, &state)?;
    let request = ScopeRequest::Files(vec![file.to_owned()]);
    let needed = engine.facts_needed(Mode::Hook);
    let (_, facts, sources) = match check::prepare(&repo, &repo.worktree(), request, &needed) {
        // A symlink that leads out of the repo.
        Err(CliError::Scope(ScopeError::Repo(RepoError::OutsideRepo(_)))) => {
            return Ok(Vec::new());
        }
        prepared => prepared?,
    };
    Ok(engine.run(sources, &facts, Mode::Hook).findings)
}

#[cfg(test)]
mod tests {
    use kit::prelude::*;
    use kit::{
        Files, GroupDef, Impl, RoleDef, Roles, RuleDef, RuleKind, RuleMeta, Ruleset, Source, build,
    };
    use state::State;
    use testkit::TempRepo;

    use super::errors;

    #[derive(Deserialize)]
    #[serde(crate = "kit::serde", deny_unknown_fields)]
    struct PerFile {}

    impl Rule for PerFile {
        fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
            cx.report(Span::empty(0));
        }
    }

    const fn per_file(id: &'static str, kind: RuleKind) -> RuleDef {
        RuleDef {
            meta: RuleMeta {
                id,
                group: "core",
                description: "",
                kind,
                fix: false,
                files: Files::All,
                source: &[Source::Personal],
                message: "found",
                agent: None,
                options: "",
            },
            imp: Impl::Check {
                nodes: &[],
                build: build::<PerFile>,
            },
        }
    }

    const NO_ROLE: RoleDef = RoleDef {
        suffixes: &[],
        dirs: &[],
    };

    static SET: Ruleset = Ruleset {
        rules: &[
            per_file("whole-error", RuleKind::Error),
            per_file("whole-candidate", RuleKind::Candidate),
        ],
        groups: &[GroupDef {
            name: "core",
            requires_package: None,
            opt_in: false,
        }],
        roles: Roles {
            tests: NO_ROLE,
            types: NO_ROLE,
            constants: NO_ROLE,
            functions: NO_ROLE,
        },
    };

    #[test]
    fn hook_runs_errors_without_candidates() {
        let t = TempRepo::new();
        t.write("src/a.ts", "export const a = 1;\n");
        let file = t.path().join("src/a.ts");
        assert!(errors(&SET, &file).unwrap().is_empty());

        State::open(&t.path().join(".git")).enable().unwrap();
        let rules: Vec<_> = errors(&SET, &file)
            .unwrap()
            .iter()
            .map(|finding| finding.rule)
            .collect();
        assert_eq!(rules, ["whole-error"]);
    }
}
