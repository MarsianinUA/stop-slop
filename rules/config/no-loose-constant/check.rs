use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// Role suffixes of the files, besides the constants files, that are a constant's home:
    /// module-private defaults of a `*.config.ts` are config, DI tokens live in a `*.provider.ts`.
    homes: Vec<String>,
}

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        if file.role == Some(Role::Constants)
            || kit::suffix_of(file.path)
                .is_some_and(|suffix| self.homes.iter().any(|home| home == suffix))
        {
            return;
        }
        // `no-constant-in-class-file` reports every constant of a file with a class, except in a
        // functions file, where the class is the stray.
        if file.role != Some(Role::Functions) && !top_classes(file).is_empty() {
            return;
        }
        for binding in top_bindings(file) {
            if binding.kind == TopKind::Constant && binding.init.is_some_and(is_literal_value) {
                cx.report(binding.span)
                    .arg("name", binding.span.source_text(file.text));
            }
        }
    }
}
