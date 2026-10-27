use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

use crate::error::io;
use crate::fixture::read_fixture;
use crate::schema::{self, Files, GroupToml, Kind, RolesToml, RuleToml};
use crate::{FolderError, Problem};

pub(crate) struct Catalog {
    pub groups: Vec<Group>,
    pub rules: Vec<RuleFolder>,
    pub roles: RolesToml,
}

pub(crate) struct Group {
    pub name: String,
    pub requires_package: Option<String>,
    pub opt_in: bool,
}

pub(crate) struct RuleFolder {
    pub group: String,
    pub id: String,
    /// The id with `_` for `-`.
    pub module: String,
    pub dir: PathBuf,
    pub description: String,
    pub kind: Kind,
    pub fix: bool,
    /// With the group default applied.
    pub files: Files,
    /// `kit::Source` variant names.
    pub sources: Vec<&'static str>,
    pub message: String,
    pub agent: Option<String>,
    /// The `[options]` table as TOML text, `""` without the table.
    pub options: String,
}

/// Rust 2024 strict and reserved keywords that a kebab-case id can turn into.
const KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate",
    "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl",
    "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "self", "static", "struct", "super", "trait", "true", "try", "type", "typeof",
    "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

pub(crate) fn scan(rules_dir: &Path) -> Result<Catalog, Vec<FolderError>> {
    let mut errors = Vec::new();

    let root = entries(rules_dir).map_err(|e| vec![FolderError::new(None, rules_dir, io(e))])?;
    let roles_path = rules_dir.join("roles.toml");
    let roles = if has_file(&root, "roles.toml") {
        read_toml(&roles_path).unwrap_or_else(|problem| {
            errors.push(FolderError::new(None, &roles_path, problem));
            RolesToml::default()
        })
    } else {
        RolesToml::default()
    };

    let mut catalog = Catalog {
        groups: Vec::new(),
        rules: Vec::new(),
        roles,
    };
    let mut ids = HashSet::new();
    for (group, _) in root.into_iter().filter(|(_, is_dir)| *is_dir) {
        let group_dir = rules_dir.join(&group);
        let (folders, files): (Vec<_>, Vec<_>) = match entries(&group_dir) {
            Ok(entries) => entries.into_iter().partition(|(_, is_dir)| *is_dir),
            Err(e) => {
                errors.push(FolderError::new(None, &group_dir, io(e)));
                continue;
            }
        };

        let mut group_toml = GroupToml::default();
        for (name, _) in files {
            let path = group_dir.join(&name);
            if name != "group.toml" {
                errors.push(FolderError::new(None, &path, Problem::ExtraFile(name)));
                continue;
            }
            match read_toml(&path) {
                Ok(toml) => group_toml = toml,
                Err(problem) => errors.push(FolderError::new(None, &path, problem)),
            }
        }

        for (id, _) in folders {
            let dir = group_dir.join(&id);
            if !ids.insert(id.clone()) {
                errors.push(FolderError::new(Some(&id), &dir, Problem::DuplicateId));
            }
            if let Some(rule) = scan_rule(&group, &id, dir, group_toml.files, &mut errors) {
                catalog.rules.push(rule);
            }
        }
        catalog.groups.push(Group {
            name: group,
            requires_package: group_toml.requires_package,
            opt_in: group_toml.opt_in,
        });
    }

    if errors.is_empty() {
        Ok(catalog)
    } else {
        Err(errors)
    }
}

/// Checks one rule folder. Returns the rule when its `rule.toml` parsed, even if other
/// checks pushed errors: the caller renders nothing while any error exists.
fn scan_rule(
    group: &str,
    id: &str,
    dir: PathBuf,
    group_files: Option<Files>,
    errors: &mut Vec<FolderError>,
) -> Option<RuleFolder> {
    let mut report = |path: &Path, problem| errors.push(FolderError::new(Some(id), path, problem));
    let module = id.replace('-', "_");
    if !is_kebab(id) {
        report(&dir, Problem::BadId);
    } else if KEYWORDS.contains(&module.as_str()) {
        report(&dir, Problem::RustKeyword);
    }

    let names = match entries(&dir) {
        Ok(names) => names,
        Err(e) => {
            report(&dir, io(e));
            return None;
        }
    };
    let mut has_fixtures = false;
    let mut invalid = Vec::new();
    for (name, is_dir) in &names {
        match name.as_str() {
            _ if *is_dir => report(&dir.join(name), Problem::ExtraFile(name.clone())),
            "rule.toml" | "check.rs" => {}
            "fixed.ts" => has_fixtures = true,
            _ if is_fixture(name, "valid") => has_fixtures = true,
            _ if is_fixture(name, "invalid") => {
                has_fixtures = true;
                invalid.push(dir.join(name));
            }
            _ => report(&dir.join(name), Problem::ExtraFile(name.clone())),
        }
    }
    let has = |file: &str| has_file(&names, file);

    let toml_path = dir.join("rule.toml");
    let toml: RuleToml = match read_toml(&toml_path) {
        Ok(toml) => toml,
        Err(problem) => {
            report(&toml_path, problem);
            return None;
        }
    };

    let mut sources = Vec::new();
    for source in &toml.source {
        match schema::source_variant(source) {
            Some(variant) => sources.push(variant),
            None => report(&toml_path, Problem::UnknownSource(source.clone())),
        }
    }

    if toml.kind == Kind::Agent {
        if has("check.rs") {
            report(&dir, Problem::AgentHasCode);
        }
        if has_fixtures {
            report(&dir, Problem::AgentHasFixtures);
        }
        if toml.fix {
            report(&toml_path, Problem::AgentHasFix);
        }
        if toml
            .agent
            .as_deref()
            .is_none_or(|hint| hint.trim().is_empty())
        {
            report(&toml_path, Problem::AgentNoHint);
        }
    } else {
        if !has("check.rs") {
            report(&dir, Problem::MissingCheck);
        }
        if !has("valid.ts") || !has("invalid.ts") {
            report(&dir, Problem::MissingFixtures);
        }
        if toml.fix != has("fixed.ts") {
            report(&dir, Problem::FixMismatch);
        }
        for file in invalid {
            match read_fixture(&file) {
                Ok(fixture) if fixture.expected.is_empty() => report(&file, Problem::NoExpect),
                Ok(fixture) => {
                    for expected in fixture.expected.into_iter().filter(|e| e.rule != id) {
                        report(&file, Problem::ForeignExpect(expected.rule));
                    }
                }
                Err(error) => report(&error.path, error.problem),
            }
        }
    }

    let options = toml.options.as_ref().map_or_else(String::new, |table| {
        toml::to_string(table).expect("a parsed TOML table serializes back")
    });
    Some(RuleFolder {
        group: group.to_string(),
        id: id.to_string(),
        module,
        dir,
        description: toml.description,
        kind: toml.kind,
        fix: toml.fix,
        files: toml.files.or(group_files).unwrap_or(Files::All),
        sources,
        message: toml.message,
        agent: toml.agent,
        options,
    })
}

/// `(name, is_dir)` of each entry, sorted by name.
fn entries(dir: &Path) -> std::io::Result<Vec<(String, bool)>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // `metadata` follows symlinks and junctions, so a linked folder still counts as one.
        entries.push((name, fs::metadata(entry.path())?.is_dir()));
    }
    entries.sort();
    Ok(entries)
}

fn has_file(entries: &[(String, bool)], file: &str) -> bool {
    entries.iter().any(|(name, is_dir)| name == file && !is_dir)
}

/// CRLF from a Windows checkout becomes LF, so texts in the registry match on every OS.
fn read_toml<T: DeserializeOwned>(path: &Path) -> Result<T, Problem> {
    let text = fs::read_to_string(path).map_err(io)?.replace("\r\n", "\n");
    toml::from_str(&text).map_err(|e| {
        let message = e.message();
        if let Some(key) = message
            .strip_prefix("unknown field `")
            .and_then(|rest| rest.split_once('`'))
            .map(|(key, _)| key)
        {
            return Problem::UnknownKey(key.to_string());
        }
        match e.span() {
            Some(span) => {
                // Bytes: slicing the `str` would panic on a span that starts inside a character.
                let line = text.as_bytes()[..span.start]
                    .iter()
                    .filter(|&&byte| byte == b'\n')
                    .count()
                    + 1;
                Problem::BadToml(format!("line {line}: {message}"))
            }
            None => Problem::BadToml(message.to_string()),
        }
    })
}

/// `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`
fn is_kebab(id: &str) -> bool {
    id.starts_with(|c: char| c.is_ascii_lowercase())
        && id.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// `<stem>.ts` or `<stem>.<name>.ts`.
fn is_fixture(name: &str, stem: &str) -> bool {
    match name
        .strip_prefix(stem)
        .and_then(|rest| rest.strip_suffix(".ts"))
    {
        Some("") => true,
        Some(name) => name.len() > 1 && name.starts_with('.'),
        None => false,
    }
}
