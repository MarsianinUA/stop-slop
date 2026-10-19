use std::path::{Component, Path};

use crate::scan::{Catalog, RuleFolder};
use crate::schema::{Kind, RoleToml};

/// Text of `registry.rs`. Every path starts with `::kit::`, every string goes through `{:?}`.
pub(crate) fn render(catalog: &Catalog) -> String {
    let mut code = String::new();
    for rule in &catalog.rules {
        code.push_str(&wrapper(rule));
    }

    let modules: Vec<String> = catalog
        .rules
        .iter()
        .map(|rule| format!("{}::DEF", rule.module))
        .collect();
    let groups: String = catalog
        .groups
        .iter()
        .map(|group| {
            format!(
                "        ::kit::GroupDef {{ name: {:?}, requires_package: {:?}, opt_in: {} }},\n",
                group.name,
                group.requires_package.as_deref(),
                group.opt_in,
            )
        })
        .collect();
    let roles = &catalog.roles;
    code.push_str(&format!(
        "pub static RULESET: ::kit::Ruleset = ::kit::Ruleset {{
    rules: &[{modules}],
    groups: &[
{groups}    ],
    roles: ::kit::Roles {{
        tests: {tests},
        types: {types},
        constants: {constants},
        functions: {functions},
    }},
}};
",
        modules = modules.join(", "),
        tests = role(&roles.tests),
        types = role(&roles.types),
        constants = role(&roles.constants),
        functions = role(&roles.functions),
    ));
    code
}

/// `#[path]` sits on the wrapper and names the rule folder: on the inner `mod check`
/// rust-analyzer cannot resolve it.
fn wrapper(rule: &RuleFolder) -> String {
    let (path, check, imp) = if rule.kind == Kind::Agent {
        (String::new(), "", "::kit::Impl::Agent")
    } else {
        (
            format!("#[path = {:?}]\n", slash_path(&rule.dir)),
            "    mod check;\n",
            "::kit::Impl::Check {
            nodes: <check::Check as ::kit::Rule>::NODES,
            build: ::kit::build::<check::Check>,
        }",
        )
    };
    let sources: Vec<String> = rule
        .sources
        .iter()
        .map(|variant| format!("::kit::Source::{variant}"))
        .collect();
    format!(
        "{path}mod {module} {{
{check}    pub(super) const DEF: ::kit::RuleDef = ::kit::RuleDef {{
        meta: ::kit::RuleMeta {{
            id: {id:?},
            group: {group:?},
            description: {description:?},
            kind: ::kit::RuleKind::{kind:?},
            fix: {fix},
            files: ::kit::Files::{files:?},
            source: &[{sources}],
            message: {message:?},
            agent: {agent:?},
            options: {options:?},
        }},
        imp: {imp},
    }};
}}
",
        module = rule.module,
        id = rule.id,
        group = rule.group,
        description = rule.description,
        kind = rule.kind,
        fix = rule.fix,
        files = rule.files,
        sources = sources.join(", "),
        message = rule.message,
        agent = rule.agent.as_deref(),
        options = rule.options,
    )
}

fn role(role: &RoleToml) -> String {
    format!(
        "::kit::RoleDef {{ suffixes: &{:?}, dirs: &{:?} }}",
        role.suffixes, role.dirs
    )
}

/// Parts joined with `/`: `display()` keeps Windows backslashes, and `canonicalize` adds
/// a `\\?\` prefix that `#[path]` rejects.
fn slash_path(dir: &Path) -> String {
    let mut path = String::new();
    for part in dir.components() {
        match part {
            Component::RootDir => path.push('/'),
            Component::Prefix(prefix) => path.push_str(&prefix.as_os_str().to_string_lossy()),
            part => {
                if !path.is_empty() && !path.ends_with('/') {
                    path.push('/');
                }
                path.push_str(&part.as_os_str().to_string_lossy());
            }
        }
    }
    path
}
