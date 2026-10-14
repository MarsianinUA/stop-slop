use kit::oxc::ast::ast_kind::AST_TYPE_MAX;
use kit::{DynRule, Files, Impl, Package, Role, Ruleset};

use crate::Mode;
use crate::config::{depends_on, requires_package};

/// The rules to run on one kind of file, as indexes into `Engine.rules`.
pub(crate) struct Profile {
    /// Indexed by `AstType as usize`.
    pub by_type: Vec<Vec<usize>>,
    pub on_file: Vec<usize>,
}

/// A profile for every mode x (Tests role or not) x (which group conditions the file's
/// package meets), built once so a file only picks its profile.
pub(crate) struct Dispatch {
    /// `requires-package` of the groups with enabled rules. Bit `i` of a profile index says
    /// whether `conditions[i]` holds; the next bit is the Tests role, the last one `Hook`.
    conditions: Vec<&'static str>,
    profiles: Vec<Profile>,
}

impl Dispatch {
    pub fn new(set: &Ruleset, rules: &[(usize, Box<dyn DynRule>)]) -> Dispatch {
        let mut conditions = Vec::new();
        for (at, _) in rules {
            if let Some(name) = requires_package(set, &set.rules[*at].meta)
                && !conditions.contains(&name)
            {
                conditions.push(name);
            }
        }
        let condition_of: Vec<Option<usize>> = rules
            .iter()
            .map(|(at, _)| {
                let name = requires_package(set, &set.rules[*at].meta)?;
                conditions.iter().position(|c| *c == name)
            })
            .collect();
        let tests_bit = conditions.len();
        let hook_bit = tests_bit + 1;

        let profiles = (0..1 << (hook_bit + 1))
            .map(|index: usize| {
                let bit = |at: usize| index >> at & 1 == 1;
                let mode = if bit(hook_bit) {
                    Mode::Hook
                } else {
                    Mode::Check
                };
                let mut profile = Profile {
                    by_type: vec![Vec::new(); AST_TYPE_MAX as usize + 1],
                    on_file: Vec::new(),
                };
                for (r, (at, _)) in rules.iter().enumerate() {
                    let def = &set.rules[*at];
                    let files = match def.meta.files {
                        Files::All => true,
                        Files::Code => !bit(tests_bit),
                        Files::Tests => bit(tests_bit),
                    };
                    let group = condition_of[r].is_none_or(bit);
                    if !(files && group && mode.runs(def.meta.kind)) {
                        continue;
                    }
                    if let Impl::Check { nodes, .. } = def.imp {
                        for ty in nodes {
                            profile.by_type[*ty as usize].push(r);
                        }
                    }
                    profile.on_file.push(r);
                }
                profile
            })
            .collect();
        Dispatch {
            conditions,
            profiles,
        }
    }

    pub fn profile(&self, mode: Mode, role: Option<Role>, package: Option<&Package>) -> &Profile {
        let tests_bit = self.conditions.len();
        let mut index = usize::from(role == Some(Role::Tests)) << tests_bit
            | usize::from(mode == Mode::Hook) << (tests_bit + 1);
        for (at, name) in self.conditions.iter().enumerate() {
            if depends_on(package, name) {
                index |= 1 << at;
            }
        }
        &self.profiles[index]
    }
}
