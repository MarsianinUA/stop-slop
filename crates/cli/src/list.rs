use std::collections::BTreeSet;

use engine::RuleState;
use kit::{Fact, Ruleset};
use project::ProjectFacts;
use repo::Repo;
use state::State;

use crate::error::{CliError, Exit};
use crate::output;

/// Without `--json` the repo is not needed, so the list prints outside one too.
pub(crate) fn run(json: bool) -> Result<Exit, CliError> {
    let text = if json {
        output::rules_json(&states(&rules::RULESET, &crate::cwd_repo()?)?)
    } else {
        output::rules_text(&rules::RULESET)
    };
    output::print(&text)?;
    Ok(Exit::Clean)
}

/// The nearest `package.json` of a `package.json` is itself, so a group condition is active
/// when it holds for at least one `package.json` of the repo.
fn states(set: &'static Ruleset, repo: &Repo) -> Result<Vec<RuleState>, CliError> {
    let state = State::open(repo.git_common_dir());
    // `ignore.json` hides findings, not rules.
    let engine = crate::new_engine(set, &state, BTreeSet::new())?;
    let snapshot = repo.worktree();
    let packages: Vec<_> = snapshot
        .files()?
        .into_iter()
        .filter(|path| path.file_name() == Some("package.json"))
        .collect();
    let facts = ProjectFacts::load(&snapshot, &packages, &[Fact::Packages])?;
    Ok(engine.rule_states(&facts, &packages))
}

#[cfg(test)]
mod tests {
    use kit::{
        Files, GroupDef, Impl, RoleDef, Roles, RuleDef, RuleKind, RuleMeta, Ruleset, Source,
    };
    use repo::Repo;
    use testkit::TempRepo;

    use super::states;

    const fn agent(id: &'static str, group: &'static str) -> RuleDef {
        RuleDef {
            meta: RuleMeta {
                id,
                group,
                description: "",
                kind: RuleKind::Agent,
                fix: false,
                files: Files::All,
                source: &[Source::Personal],
                message: "",
                agent: Some("look"),
                options: "",
            },
            imp: Impl::Agent,
        }
    }

    const NO_ROLE: RoleDef = RoleDef {
        suffixes: &[],
        dirs: &[],
    };

    static SET: Ruleset = Ruleset {
        rules: &[agent("nest-rule", "nest"), agent("plain-rule", "plain")],
        groups: &[
            GroupDef {
                name: "nest",
                requires_package: Some("@nestjs/core"),
            },
            GroupDef {
                name: "plain",
                requires_package: None,
            },
        ],
        roles: Roles {
            tests: NO_ROLE,
            types: NO_ROLE,
            constants: NO_ROLE,
            functions: NO_ROLE,
        },
    };

    const NEST: &str = r#"{"dependencies": {"@nestjs/core": "11.0.0"}}"#;

    fn active(t: &TempRepo) -> Vec<(&'static str, bool)> {
        let repo = Repo::open(t.path()).unwrap();
        states(&SET, &repo)
            .unwrap()
            .iter()
            .map(|state| (state.meta.id, state.active))
            .collect()
    }

    #[test]
    fn group_holds_when_one_package_json_has_the_package() {
        let t = TempRepo::new();
        t.write("package.json", "{}")
            .write("apps/api/package.json", NEST)
            .write("apps/web/package.json", "{}");
        assert_eq!(active(&t), [("nest-rule", true), ("plain-rule", true)]);

        t.write("apps/api/package.json", "{}");
        assert_eq!(active(&t), [("nest-rule", false), ("plain-rule", true)]);

        t.write("package.json", NEST);
        assert_eq!(active(&t), [("nest-rule", true), ("plain-rule", true)]);
    }

    #[test]
    fn group_is_off_without_package_json() {
        let t = TempRepo::new();
        assert_eq!(active(&t), [("nest-rule", false), ("plain-rule", true)]);
    }

    #[test]
    fn disabled_rule_is_inactive() {
        let t = TempRepo::new();
        t.write(
            ".git/info/stop-slop/config.toml",
            "disabled = [\"plain-rule\"]\n",
        );
        assert_eq!(active(&t), [("nest-rule", false), ("plain-rule", false)]);
    }
}
