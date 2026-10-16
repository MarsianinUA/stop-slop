use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        // In a functions or constants file the class is the stray, and `no-class-in-function-file`
        // reports it.
        if matches!(file.role, Some(Role::Functions | Role::Constants)) {
            return;
        }
        let Some(class) = top_classes(file).first().map(|class| class.name) else {
            return;
        };
        for binding in top_bindings(file) {
            if binding.kind == TopKind::Constant {
                cx.report(binding.span)
                    .arg("name", binding.span.source_text(file.text))
                    .arg("class", class);
            }
        }
    }
}
