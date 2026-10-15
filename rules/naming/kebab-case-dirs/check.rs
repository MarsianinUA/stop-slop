use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let Some(parent) = file.path.parent() else {
            return;
        };
        let dirs: Vec<String> = parent
            .iter()
            .filter(|dir| !passes(dir))
            .map(|dir| format!("`{dir}`"))
            .collect();
        // One finding for all folders: they have no text in the file, so findings of their own
        // would share one fingerprint.
        if !dirs.is_empty() {
            cx.report(Span::empty(code_start(file)))
                .arg("dirs", dirs.join(", "));
        }
    }
}

/// A name with other characters is framework syntax and passes: `[orderId]`, `(shop)`,
/// `@modal`. One leading `.` (`.well-known`) and a `__..__` wrapper (`__tests__`) are dropped.
fn passes(dir: &str) -> bool {
    if !dir
        .chars()
        .all(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return true;
    }
    let dir = dir.strip_prefix('.').unwrap_or(dir);
    let dir = dir
        .strip_prefix("__")
        .and_then(|inner| inner.strip_suffix("__"))
        .unwrap_or(dir);
    Case::Kebab.matches(dir)
}

/// The path has no text in the file, so the finding sits where the code starts: a fixture's
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
