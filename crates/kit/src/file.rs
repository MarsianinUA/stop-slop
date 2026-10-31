use camino::Utf8Path;
use oxc_semantic::Semantic;
use oxc_span::{GetSpan, Span};

use crate::{LineIndex, words};

pub struct File<'a> {
    /// Relative to the repo root, with `/`.
    pub path: &'a Utf8Path,
    /// Without BOM; line endings as in the file.
    pub text: &'a str,
    pub lines: &'a LineIndex,
    pub semantic: &'a Semantic<'a>,
    pub role: Option<Role>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Tests,
    Types,
    Constants,
    Functions,
}

#[derive(Debug)]
pub struct Roles {
    pub tests: RoleDef,
    pub types: RoleDef,
    pub constants: RoleDef,
    pub functions: RoleDef,
}

#[derive(Debug)]
pub struct RoleDef {
    pub suffixes: &'static [&'static str],
    pub dirs: &'static [&'static str],
}

impl Roles {
    /// The first matching role wins: `__tests__/a.types.ts` is Tests, not Types.
    pub fn role_of(&self, path: &Utf8Path) -> Option<Role> {
        let suffix = suffix_of(path);
        let dirs = path.parent().unwrap_or(Utf8Path::new(""));
        [
            (Role::Tests, &self.tests),
            (Role::Types, &self.types),
            (Role::Constants, &self.constants),
            (Role::Functions, &self.functions),
        ]
        .into_iter()
        .find(|(_, def)| {
            suffix.is_some_and(|suffix| def.suffixes.contains(&suffix))
                || dirs.iter().any(|dir| def.dirs.contains(&dir))
        })
        .map(|(role, _)| role)
    }
}

/// The extension of the file stem: `order.response.ts` -> `response`;
/// `a.ts` and dotfiles like `.types.ts` -> `None`.
pub fn suffix_of(path: &Utf8Path) -> Option<&str> {
    Utf8Path::new(path.file_stem()?).extension()
}

/// Where a finding about the file's path sits: an empty span where the code starts. The path has
/// no text in the file; a fixture's `// expect:` mark can reach that line, and a hunk that adds
/// the file covers it.
pub(crate) fn path_span(file: &File<'_>) -> Span {
    let program = file.semantic.nodes().program();
    let start = program
        .directives
        .first()
        .map(|directive| directive.span.start)
        .or_else(|| program.body.first().map(|statement| statement.span().start))
        .unwrap_or(0);
    Span::empty(start)
}

/// A command run from a shell: the file starts with `#!`, or a folder on its path is one of
/// `dirs` (`scripts/seed.ts`). Its top level is its `main`, its output the console.
pub fn is_cli_script(file: &File<'_>, dirs: &[String]) -> bool {
    file.semantic.nodes().program().hashbang.is_some()
        || file
            .path
            .parent()
            .is_some_and(|parent| parent.iter().any(|dir| dirs.iter().any(|d| d == dir)))
}

/// A word of the file's name is one of `list`, ignoring case: each part between dots goes through
/// `words` (`require-env.ts` holds `env`, `order.response.dto.ts` holds `response`).
pub fn file_name_has_word(file: &File<'_>, list: &[String]) -> bool {
    file.path.file_stem().is_some_and(|stem| {
        stem.split('.')
            .flat_map(words)
            .any(|word| list.iter().any(|item| item.eq_ignore_ascii_case(word)))
    })
}
