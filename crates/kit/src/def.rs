use oxc_ast::AstType;

use crate::{DynRule, Roles};

#[derive(Debug)]
pub struct Ruleset {
    pub rules: &'static [RuleDef],
    pub groups: &'static [GroupDef],
    pub roles: Roles,
}

#[derive(Debug)]
pub struct RuleDef {
    pub meta: RuleMeta,
    pub imp: Impl,
}

#[derive(Debug)]
pub struct RuleMeta {
    pub id: &'static str,
    pub group: &'static str,
    pub description: &'static str,
    pub kind: RuleKind,
    pub fix: bool,
    /// The rule's own `files`, else its group's, else `All`.
    pub files: Files,
    pub source: &'static [Source],
    pub message: &'static str,
    pub agent: Option<&'static str>,
    /// The `[options]` table of `rule.toml` as TOML text.
    pub options: &'static str,
}

#[derive(Debug)]
pub enum Impl {
    Agent,
    Check {
        nodes: &'static [AstType],
        build: Build,
    },
}

pub type Build = fn(options: &str) -> Result<Box<dyn DynRule>, toml::de::Error>;

#[derive(Debug)]
pub struct GroupDef {
    pub name: &'static str,
    pub requires_package: Option<&'static str>,
    /// Off unless `enabled` of `config.toml` names the group.
    pub opt_in: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleKind {
    Error,
    Candidate,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingKind {
    Error,
    Candidate,
    ParseError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Files {
    All,
    Code,
    Tests,
}

/// Where the rule comes from: `Personal` - own conventions; `Direct` - the `direct-taste`,
/// `direct-principles` and `design-language` skills; `Project` - a project's `CONVENTIONS.md`,
/// general enough for any project; `AntiSlop` - ported from dmmulroy/anti-slop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Personal,
    Direct,
    Project,
    AntiSlop,
}
