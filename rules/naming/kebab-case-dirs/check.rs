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
            cx.report(path_span(file)).arg("dirs", dirs.join(", "));
        }
    }
}

/// A name with other characters is framework syntax and passes: `[orderId]`, `(shop)`,
/// `@modal`. One leading `.` (`.well-known`), a `__..__` wrapper (`__tests__`) or else one
/// leading `_` (`_components`, a private folder of Next) is dropped.
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
        .or_else(|| dir.strip_prefix('_'))
        .unwrap_or(dir);
    Case::Kebab.matches(dir)
}
