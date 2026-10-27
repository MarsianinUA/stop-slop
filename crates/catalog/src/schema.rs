use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuleToml {
    pub description: String,
    pub kind: Kind,
    #[serde(default)]
    pub fix: bool,
    pub files: Option<Files>,
    /// Plain strings, so an unknown value becomes `UnknownSource` instead of a bare TOML error.
    pub source: Vec<String>,
    pub message: String,
    pub agent: Option<String>,
    pub options: Option<toml::Table>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct GroupToml {
    pub files: Option<Files>,
    pub requires_package: Option<String>,
    #[serde(default)]
    pub opt_in: bool,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct RolesToml {
    pub tests: RoleToml,
    pub types: RoleToml,
    pub constants: RoleToml,
    pub functions: RoleToml,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct RoleToml {
    pub suffixes: Vec<String>,
    pub dirs: Vec<String>,
}

/// `Debug` prints the `kit::RuleKind` variant name.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    Error,
    Candidate,
    Agent,
}

/// `Debug` prints the `kit::Files` variant name.
#[derive(Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Files {
    All,
    Code,
    Tests,
}

/// The `kit::Source` variant for a `source` value of `rule.toml`.
pub(crate) fn source_variant(source: &str) -> Option<&'static str> {
    match source {
        "personal" => Some("Personal"),
        "direct" => Some("Direct"),
        "project" => Some("Project"),
        "anti-slop" => Some("AntiSlop"),
        _ => None,
    }
}
