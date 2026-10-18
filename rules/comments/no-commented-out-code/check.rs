use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        // A doc comment is documentation: an example in it is there on purpose.
        for block in comment_blocks(file).into_iter().filter(|block| !block.doc) {
            for part in block.parts {
                if part.kind == CommentKind::Code {
                    let first = one_line(part.lines[0].source_text(file.text));
                    cx.report(part.span).arg("code", first);
                }
            }
        }
    }
}
