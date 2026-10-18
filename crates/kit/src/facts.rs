use std::collections::{BTreeSet, HashMap};

use camino::{Utf8Path, Utf8PathBuf};

pub trait Facts: Sync {
    /// The nearest `package.json`, with the packages of every `package.json` above it up to the
    /// repo root: Node looks for a bare import in `node_modules` from the folder of the file up.
    fn package(&self, file: &Utf8Path) -> Option<&Package>;
    /// Keys of `paths` in the nearest `tsconfig.json`.
    fn aliases(&self, file: &Utf8Path) -> &[String];
    fn enums(&self) -> &EnumIndex;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fact {
    Packages,
    Aliases,
    Enums,
}

#[derive(Debug)]
pub struct Package {
    /// Keys of `dependencies`, `devDependencies`, `peerDependencies` and
    /// `optionalDependencies` of the nearest `package.json` and of every one above it.
    pub dependencies: BTreeSet<String>,
    /// Packages of the same monorepo (`workspace:` dependencies).
    pub workspace: BTreeSet<String>,
}

#[derive(Debug)]
pub struct EnumMember {
    pub enum_name: String,
    pub member: String,
    pub file: Utf8PathBuf,
}

#[derive(Debug)]
pub struct EnumIndex {
    by_value: HashMap<String, Vec<EnumMember>>,
}

impl EnumIndex {
    /// Each `String` is the raw value the member is looked up by in `by_value`.
    pub fn new(members: Vec<(String, EnumMember)>) -> EnumIndex {
        let mut by_value: HashMap<String, Vec<EnumMember>> = HashMap::new();
        for (raw, member) in members {
            by_value.entry(raw).or_default().push(member);
        }
        EnumIndex { by_value }
    }

    pub fn by_value(&self, raw: &str) -> &[EnumMember] {
        self.by_value.get(raw).map_or(&[], Vec::as_slice)
    }
}

/// Facts given up front, for tests and rule fixtures.
#[derive(Debug)]
pub struct StaticFacts {
    pub package: Option<Package>,
    pub aliases: Vec<String>,
    pub enums: EnumIndex,
}

impl Facts for StaticFacts {
    fn package(&self, _file: &Utf8Path) -> Option<&Package> {
        self.package.as_ref()
    }

    fn aliases(&self, _file: &Utf8Path) -> &[String] {
        &self.aliases
    }

    fn enums(&self) -> &EnumIndex {
        &self.enums
    }
}
