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
            cx.report_path()
                .arg("name", name)
                .arg("example", self.example(stem, file.path.extension()));
        }
    }
}

impl Check {
    /// The name in kebab-case, with a `<role>` the agent picks where it has none:
    /// `orderTotals.ts` -> `order-totals.<role>.ts`.
    fn example(&self, stem: &str, extension: Option<&str>) -> String {
        let mut parts: Vec<String> = stem
            .split('.')
            .map(|part| Case::Kebab.apply(part).unwrap_or_else(|| part.to_owned()))
            .collect();
        if let [only] = parts.as_slice()
            && !self.names.contains(only)
        {
            parts.push("<role>".to_owned());
        }
        if let Some(extension) = extension {
            parts.push(extension.to_owned());
        }
        parts.join(".")
    }
}
