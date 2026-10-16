use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let role = match file.role {
            Some(Role::Functions) => "functions",
            Some(Role::Constants) => "constants",
            _ => return,
        };
        for class in top_classes(file) {
            cx.report(class.head)
                .arg("name", class.name)
                .arg("role", role);
        }
    }
}
