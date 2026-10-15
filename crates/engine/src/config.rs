use camino::Utf8PathBuf;
use kit::{Fact, Facts, Impl, Package, RuleMeta, Ruleset};

use crate::{Dispatch, Engine, Mode, RuleState, RunConfig};

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// `key` is `disabled[2]` or `rules.no-such`.
    #[error("unknown rule `{key}`")]
    UnknownRule { key: String },
    /// `key` is `rules.<id>.<option>`.
    #[error("unknown option `{key}`")]
    UnknownOption { key: String },
    #[error("bad option `{key}`: {message}")]
    BadOption { key: String, message: String },
}

impl Engine {
    /// Returns the first error: `disabled`, then unknown `rules.*` ids, then options.
    pub fn new(set: &'static Ruleset, config: RunConfig) -> Result<Engine, ConfigError> {
        let known = |id: &str| set.rules.iter().any(|def| def.meta.id == id);
        if let Some(i) = config.disabled.iter().position(|id| !known(id)) {
            return Err(ConfigError::UnknownRule {
                key: format!("disabled[{i}]"),
            });
        }
        if let Some(id) = config.options.keys().find(|id| !known(id)) {
            return Err(ConfigError::UnknownRule {
                key: format!("rules.{id}"),
            });
        }

        let enabled: Vec<bool> = set
            .rules
            .iter()
            .map(|def| !config.disabled.iter().any(|id| id == def.meta.id))
            .collect();
        let mut rules = Vec::new();
        for (def, &enabled) in set.rules.iter().zip(&enabled) {
            let overrides = config.options.get(def.meta.id);
            let options = options(&def.meta, overrides)?;
            // `[rules.<id>]` of a disabled rule is built too, so a bad value shows before the
            // rule is turned back on, as an unknown key does.
            if let Impl::Check { build, .. } = &def.imp
                && (enabled || overrides.is_some())
            {
                let rule =
                    build(&options).map_err(|error| bad_option(&def.meta, &options, &error))?;
                if enabled {
                    rules.push((def, rule));
                }
            }
        }
        Ok(Engine {
            set,
            dispatch: Dispatch::new(set, &rules),
            enabled,
            rules,
            ignored: config.ignored,
        })
    }

    /// `uses()` of the enabled rules, plus `Packages` when a group of one of them has
    /// `requires-package`; in `Hook` only `kind = error` rules count, in `Fix` only
    /// `fix = true` ones.
    pub fn facts_needed(&self, mode: Mode) -> Vec<Fact> {
        let mut needed: Vec<Fact> = self
            .rules
            .iter()
            .filter(|(def, _)| mode.runs(&def.meta))
            .flat_map(|(_, rule)| rule.uses())
            .copied()
            .collect();
        let conditional = self
            .set
            .rules
            .iter()
            .zip(&self.enabled)
            .any(|(def, &enabled)| {
                enabled && mode.runs(&def.meta) && requires_package(self.set, &def.meta).is_some()
            });
        if conditional {
            needed.push(Fact::Packages);
        }
        // Exhaustive, so a new `Fact` cannot be dropped silently.
        needed.sort_by_key(|fact| match fact {
            Fact::Packages => 0,
            Fact::Aliases => 1,
            Fact::Enums => 2,
        });
        needed.dedup();
        needed
    }

    /// Every rule of the set, `agent` too. Active: not disabled, and the group condition holds
    /// for at least one of `files`.
    pub fn rule_states(&self, facts: &dyn Facts, files: &[Utf8PathBuf]) -> Vec<RuleState> {
        let packages: Vec<Option<&Package>> =
            files.iter().map(|file| facts.package(file)).collect();
        self.set
            .rules
            .iter()
            .zip(&self.enabled)
            .map(|(def, &enabled)| RuleState {
                meta: &def.meta,
                active: enabled
                    && requires_package(self.set, &def.meta).is_none_or(|name| {
                        packages.iter().any(|&package| depends_on(package, name))
                    }),
            })
            .collect()
    }
}

pub(crate) fn requires_package(set: &Ruleset, meta: &RuleMeta) -> Option<&'static str> {
    set.groups
        .iter()
        .find(|group| group.name == meta.group)?
        .requires_package
}

pub(crate) fn depends_on(package: Option<&Package>, name: &str) -> bool {
    package.is_some_and(|package| package.dependencies.contains(name))
}

/// `[options]` of `rule.toml` with `[rules.<id>]` of `config.toml` on top, as TOML text.
fn options(meta: &RuleMeta, overrides: Option<&toml::Table>) -> Result<String, ConfigError> {
    let Some(overrides) = overrides else {
        return Ok(meta.options.to_string());
    };
    let mut table: toml::Table =
        toml::from_str(meta.options).expect("catalog checked [options] of rule.toml");
    if let Some(key) = overrides.keys().find(|key| !table.contains_key(*key)) {
        return Err(ConfigError::UnknownOption {
            key: format!("rules.{}.{key}", meta.id),
        });
    }
    table.extend(overrides.clone());
    Ok(toml::to_string(&table).expect("a parsed TOML table serializes back"))
}

/// Names the option on the line the error points at; just the rule when it points at none.
fn bad_option(meta: &RuleMeta, options: &str, error: &toml::de::Error) -> ConfigError {
    let option = error.span().and_then(|span| {
        // Bytes: slicing the `str` would panic on a span that starts inside a character.
        let line_start = options.as_bytes()[..span.start]
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |at| at + 1);
        let line = options[line_start..].lines().next()?;
        Some(line.split_once('=')?.0.trim())
    });
    ConfigError::BadOption {
        key: match option {
            Some(option) => format!("rules.{}.{option}", meta.id),
            None => format!("rules.{}", meta.id),
        },
        message: error.message().to_string(),
    }
}
