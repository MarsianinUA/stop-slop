use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let names: Vec<String> = class_file_classes(file)
            .iter()
            .map(|class| format!("`{}`", class.name))
            .collect();
        let classes = match names.as_slice() {
            [] => return,
            [name] => format!("class {name}"),
            _ => format!("classes {}", names.join(", ")),
        };
        for binding in top_bindings(file) {
            if binding.kind == TopKind::Constant {
                cx.report(binding.span)
                    .arg("name", binding.span.source_text(file.text))
                    .arg("classes", &classes);
            }
        }
    }
}
