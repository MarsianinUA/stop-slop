// A test crate of its own: the `forbid` in lib.rs does not reach it, and it reads rule folders.
#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use catalog::{Fixture, read_fixture};
use engine::{Engine, Finding, Mode, RunConfig, RunReport, SourceFile};
use kit::{Build, EnumIndex, Fact, Impl, Package, RuleDef, StaticFacts};
use libtest_mimic::{Arguments, Failed, Trial};
use rules::RULESET;
use similar::TextDiff;

fn main() {
    let args = Arguments::from_args();
    let mut trials = Vec::new();
    for def in RULESET.rules {
        let Impl::Check { build, .. } = def.imp else {
            continue;
        };
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(def.meta.group)
            .join(def.meta.id);
        let prefix = format!("{}/{}", def.meta.group, def.meta.id);
        for (stem, file, valid) in fixtures(&dir) {
            trials.push(Trial::test(format!("{prefix}/{stem}"), move || {
                run_fixture(def, &file, valid)
            }));
        }
        if def.meta.fix {
            let fix_dir = dir.clone();
            trials.push(Trial::test(format!("{prefix}/fix"), move || {
                fix(def, &fix_dir)
            }));
            trials.push(Trial::test(format!("{prefix}/fix-again"), move || {
                fix_again(def, &dir)
            }));
        }
        trials.push(Trial::test(format!("{prefix}/options"), move || {
            options(def, build)
        }));
    }
    libtest_mimic::run(&args, trials).exit();
}

/// `(stem, path, valid)` of `valid.ts`, `valid.<name>.ts`, `invalid.ts`, `invalid.<name>.ts`.
fn fixtures(dir: &Path) -> Vec<(String, PathBuf, bool)> {
    let mut found: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| entry.expect("a readable directory entry").path())
        .filter_map(|path| {
            let stem = path.file_name()?.to_str()?.strip_suffix(".ts")?.to_string();
            let valid = match stem.split('.').next()? {
                "valid" => true,
                "invalid" => false,
                _ => return None,
            };
            Some((stem, path, valid))
        })
        .collect();
    found.sort();
    found
}

fn run_fixture(def: &'static RuleDef, file: &Path, valid: bool) -> Result<(), Failed> {
    let fixture = read_fixture(file)?;
    if valid && !fixture.expected.is_empty() {
        return Err("a valid fixture has `// expect:` marks".into());
    }
    let findings = run(def, &fixture, Mode::Check)?.findings;

    let mut expected: BTreeMap<(u32, &str), u32> = BTreeMap::new();
    for mark in &fixture.expected {
        *expected.entry((mark.line, &mark.rule)).or_default() += mark.count;
    }
    let mut got: BTreeMap<(u32, &str), u32> = BTreeMap::new();
    for finding in &findings {
        *got.entry((finding.start.line, finding.rule)).or_default() += 1;
    }
    if expected != got {
        let marks = expected
            .iter()
            .map(|((line, rule), count)| format!("  {line} {rule} x{count}"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!("expected:\n{marks}\ngot:\n{}", list(&findings)).into());
    }
    Ok(())
}

fn fix(def: &'static RuleDef, dir: &Path) -> Result<(), Failed> {
    let invalid = read_fixture(&dir.join("invalid.ts"))?;
    let fixed = read_fixture(&dir.join("fixed.ts"))?;
    if (
        &invalid.path,
        &invalid.aliases,
        &invalid.workspace,
        &invalid.dependencies,
    ) != (
        &fixed.path,
        &fixed.aliases,
        &fixed.workspace,
        &fixed.dependencies,
    ) {
        return Err(concat!(
            "fixed.ts needs the same `// @path`, `// @alias`, `// @workspace`, `// @dependency` ",
            "headers as invalid.ts"
        )
        .into());
    }
    if !fixed.expected.is_empty() {
        return Err("fixed.ts has `// expect:` marks; findings are checked in invalid.ts".into());
    }
    // Without this, a fixed.ts copied from invalid.ts would pass.
    let Some(got) = fixed_text(def, &invalid)? else {
        return Err(concat!(
            "`fix = true`, but the fix of invalid.ts changes nothing: no `.fix(..)`, or the engine ",
            "dropped the edits (they overlap, cut a character, write the text that is already ",
            "there or leave text that does not parse)"
        )
        .into());
    };
    if got != fixed.text {
        let diff = diff(&fixed.text, &got, "fix(invalid.ts)");
        return Err(format!("fix(invalid.ts) differs from fixed.ts:\n{diff}").into());
    }
    Ok(())
}

fn fix_again(def: &'static RuleDef, dir: &Path) -> Result<(), Failed> {
    let fixed = read_fixture(&dir.join("fixed.ts"))?;
    if let Some(got) = fixed_text(def, &fixed)? {
        let diff = diff(&fixed.text, &got, "fix(fixed.ts)");
        return Err(format!("fix(fixed.ts) changes it again:\n{diff}").into());
    }
    Ok(())
}

/// The text `Fix` turns the fixture into, or `None` when it changes nothing.
fn fixed_text(def: &'static RuleDef, fixture: &Fixture) -> Result<Option<String>, Failed> {
    let file = run(def, fixture, Mode::Fix)?.rewritten.pop();
    Ok(file.map(|file| String::from_utf8(file.bytes)).transpose()?)
}

fn diff(expected: &str, got: &str, got_name: &str) -> String {
    TextDiff::from_lines(expected, got)
        .unified_diff()
        .header("fixed.ts", got_name)
        .to_string()
}

/// The fixture through an engine with only this rule enabled.
fn run(def: &'static RuleDef, fixture: &Fixture, mode: Mode) -> Result<RunReport, Failed> {
    let engine = Engine::new(
        &RULESET,
        RunConfig {
            disabled: RULESET
                .rules
                .iter()
                .map(|other| other.meta.id)
                .filter(|&id| id != def.meta.id)
                .map(str::to_string)
                .collect(),
            ..RunConfig::default()
        },
    )?;
    let facts = facts(def, &engine, fixture, mode)?;
    let file = SourceFile {
        path: fixture.path.clone(),
        bytes: fixture.text.clone().into_bytes(),
        changed: None,
    };
    Ok(engine.run(vec![file], &facts, mode))
}

/// The facts the engine asks for, with the group condition met. The enum index comes from the
/// fixture's own text. A fixture header for a fact the engine does not load fails the case:
/// the rule would see nothing in real runs.
fn facts(
    def: &RuleDef,
    engine: &Engine,
    fixture: &Fixture,
    mode: Mode,
) -> Result<StaticFacts, Failed> {
    let needed = engine.facts_needed(mode);
    let undeclared = |fact: Fact, header: &str| {
        format!(
            "the fixture has `// @{header}`, but the rule does not declare `Fact::{fact:?}` in `USES`"
        )
    };
    if !fixture.aliases.is_empty() && !needed.contains(&Fact::Aliases) {
        return Err(undeclared(Fact::Aliases, "alias").into());
    }
    if !fixture.workspace.is_empty() && !needed.contains(&Fact::Packages) {
        return Err(undeclared(Fact::Packages, "workspace").into());
    }
    if !fixture.dependencies.is_empty() && !needed.contains(&Fact::Packages) {
        return Err(undeclared(Fact::Packages, "dependency").into());
    }
    let requires_package = RULESET
        .groups
        .iter()
        .find(|group| group.name == def.meta.group)
        .and_then(|group| group.requires_package);
    Ok(StaticFacts {
        package: needed.contains(&Fact::Packages).then(|| Package {
            dependencies: requires_package
                .into_iter()
                .map(str::to_string)
                .chain(fixture.dependencies.iter().cloned())
                .collect(),
            workspace: fixture.workspace.iter().cloned().collect(),
        }),
        aliases: fixture.aliases.clone(),
        enums: if needed.contains(&Fact::Enums) {
            project::enum_index(&[(fixture.path.as_path(), fixture.text.as_str())])
        } else {
            EnumIndex::new(Vec::new())
        },
    })
}

fn options(def: &RuleDef, build: Build) -> Result<(), Failed> {
    let id = def.meta.id;
    build(def.meta.options)
        .map(drop)
        .map_err(|e| format!("{id}: [options] in rule.toml does not fit check.rs: {e}").into())
}

fn list(findings: &[Finding]) -> String {
    findings
        .iter()
        .map(|f| {
            format!(
                "  {}:{} {} {}",
                f.start.line, f.start.column, f.rule, f.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
