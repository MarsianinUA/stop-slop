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
        // A stem with other characters is framework syntax, as for folders: `[orderId].tsx`,
        // `+page.ts`. `names` cannot list a dynamic one.
        if !stem
            .chars()
            .all(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        {
            return;
        }
        let no_role = !stem.contains('.') && !self.names.iter().any(|n| n == stem);
        if no_role || !stem.split('.').all(|part| Case::Kebab.matches(part)) {
            cx.report(path_span(file)).arg("name", name);
        }
    }
}
