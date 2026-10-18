use kit::oxc::ast::ast_kind::AST_TYPE_MAX;
use kit::{DynRule, Files, Impl, Package, Role, RuleDef, Ruleset};

use crate::Mode;
use crate::config::{depends_on, requires_package};

/// The rules to run on one kind of file, as indexes into `Engine.rules`.
pub(crate) struct Profile {
    /// Indexed by `AstType as usize`.
    pub by_type: Vec<Vec<usize>>,
    /// Every rule of the profile; each gets `on_file`.
    pub rules: Vec<usize>,
}

/// A profile for every mode x (Tests role or not) x (which group conditions the file's
/// package meets), built once so a file only picks its profile.
pub(crate) struct Dispatch {
    /// `requires-package` of the groups with enabled rules. Bit `i` of a profile index says
    /// whether `conditions[i]` holds; the next bit is the Tests role, the bits above it are
    /// `mode as usize`.
    conditions: Vec<&'static str>,
    profiles: Vec<Profile>,
}

impl Dispatch {
    pub fn new(set: &Ruleset, rules: &[(&RuleDef, Box<dyn DynRule>)]) -> Dispatch {
        let mut conditions = Vec::new();
        for (def, _) in rules {
            if let Some(name) = requires_package(set, &def.meta)
                && !conditions.contains(&name)
            {
                conditions.push(name);
            }
        }
        let condition_of: Vec<Option<usize>> = rules
            .iter()
            .map(|(def, _)| {
                let name = requires_package(set, &def.meta)?;
                conditions.iter().position(|c| *c == name)
            })
            .collect();
        let tests_bit = conditions.len();
        let mode_shift = tests_bit + 1;

        let profiles = (0..Mode::ALL.len() << mode_shift)
            .map(|index: usize| {
                let bit = |at: usize| index >> at & 1 == 1;
                let mode = Mode::ALL[index >> mode_shift];
                let mut profile = Profile {
                    by_type: vec![Vec::new(); AST_TYPE_MAX as usize + 1],
                    rules: Vec::new(),
                };
                for (r, (def, _)) in rules.iter().enumerate() {
                    let files = match def.meta.files {
                        Files::All => true,
                        Files::Code => !bit(tests_bit),
                        Files::Tests => bit(tests_bit),
                    };
                    let group = condition_of[r].is_none_or(bit);
                    if !(files && group && mode.runs(&def.meta)) {
                        continue;
                    }
                    if let Impl::Check { nodes, .. } = def.imp {
                        for ty in nodes {
                            profile.by_type[*ty as usize].push(r);
                        }
                    }
                    profile.rules.push(r);
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
            | (mode as usize) << (tests_bit + 1);
        for (at, name) in self.conditions.iter().enumerate() {
            if depends_on(package, name) {
                index |= 1 << at;
            }
        }
        &self.profiles[index]
    }
}
