use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// File stems that need no role suffix: `index.ts` barrels, the `main.ts` entry.
    names: Vec<String>,
}

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let (Some(name), Some(stem)) = (file.path.file_name(), file.path.file_stem()) else {
            return;
        };
        let no_role = !stem.contains('.') && !self.names.iter().any(|n| n == stem);
        if no_role || !stem.split('.').all(|part| Case::Kebab.matches(part)) {
            cx.report(Span::empty(code_start(file))).arg("name", name);
        }
    }
}

/// The name has no text in the file, so the finding sits where the code starts: a fixture's
/// `// expect:` mark can reach that line, and a hunk that adds the file covers it.
fn code_start(file: &File<'_>) -> u32 {
    let program = file.semantic.nodes().program();
    program
        .directives
        .first()
        .map(|directive| directive.span.start)
        .or_else(|| program.body.first().map(|statement| statement.span().start))
        .unwrap_or(0)
}
