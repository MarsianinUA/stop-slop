use camino::{Utf8Path, Utf8PathBuf};
use kit::oxc::allocator::Allocator;
use kit::oxc::ast::ast::{Declaration, Statement};
use kit::oxc::parser::{ParseOptions, Parser};
use kit::oxc::span::SourceType;
use kit::{EnumIndex, EnumMember, static_text};
use repo::{RepoError, Snapshot};

const EXTENSIONS: [&str; 4] = [".ts", ".tsx", ".mts", ".cts"];
/// Generated declarations would repeat the members of their sources.
const DECLARATIONS: [&str; 3] = [".d.ts", ".d.mts", ".d.cts"];

/// String members of the enums declared at the top level of each source, in the order of
/// the sources. A source with an unknown extension is skipped.
pub fn enum_index(sources: &[(&Utf8Path, &str)]) -> EnumIndex {
    let mut allocator = Allocator::default();
    let mut members = Vec::new();
    for &(path, text) in sources {
        let Ok(source_type) = SourceType::from_path(path) else {
            continue;
        };
        collect(&allocator, path, text, source_type, &mut members);
        allocator.reset();
    }
    EnumIndex::new(members)
}

pub(crate) fn load(snapshot: &Snapshot) -> Result<EnumIndex, RepoError> {
    let mut files = snapshot.grep_word("enum")?;
    files.retain(|path| {
        let name = path.as_str();
        EXTENSIONS.iter().any(|extension| name.ends_with(extension))
            && !DECLARATIONS.iter().any(|suffix| name.ends_with(suffix))
    });
    let bytes = snapshot.read(&files)?;
    let texts: Vec<(&Utf8PathBuf, String)> = files
        .iter()
        .zip(bytes)
        .filter_map(|(path, bytes)| Some((path, String::from_utf8(bytes?).ok()?)))
        .collect();
    let sources: Vec<(&Utf8Path, &str)> = texts
        .iter()
        .map(|(path, text)| (path.as_path(), text.as_str()))
        .collect();
    Ok(enum_index(&sources))
}

/// A parse error is not an error of `load`: what the parser gave back is taken (an empty
/// body after a fatal error), so a file being edited right now does not break the others.
fn collect(
    allocator: &Allocator,
    path: &Utf8Path,
    text: &str,
    source_type: SourceType,
    members: &mut Vec<(String, EnumMember)>,
) {
    let program = Parser::new(allocator, text, source_type)
        .with_options(ParseOptions {
            preserve_parens: false,
            ..ParseOptions::default()
        })
        .parse()
        .program;
    for statement in &program.body {
        let declaration = match statement {
            Statement::TSEnumDeclaration(declaration) => declaration,
            Statement::ExportDeclaration(export) => match &export.declaration {
                Declaration::TSEnumDeclaration(declaration) => declaration,
                _ => continue,
            },
            _ => continue,
        };
        for member in &declaration.body.members {
            let Some(value) = member.initializer.as_ref().and_then(static_text) else {
                continue;
            };
            let member = EnumMember {
                enum_name: declaration.id.name.to_string(),
                member: member.id.static_name().to_string(),
                file: path.to_owned(),
            };
            members.push((value.to_string(), member));
        }
    }
}
