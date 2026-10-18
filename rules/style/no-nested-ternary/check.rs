use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let nodes = file.semantic.nodes();
        // One finding on the outermost `?:` of each nest.
        let mut outermost: Vec<Span> = Vec::new();
        for node in nodes.iter() {
            if !matches!(node.kind(), AstKind::ConditionalExpression(_)) {
                continue;
            }
            let mut outer = None;
            for ancestor in nodes.ancestors(node.id()) {
                match ancestor.kind() {
                    AstKind::ConditionalExpression(_) => outer = Some(ancestor.span()),
                    // A callback or a class in a branch has expressions of its own.
                    kind if kind.is_function_like() || matches!(kind, AstKind::Class(_)) => break,
                    _ => {}
                }
            }
            if let Some(span) = outer.filter(|span| !outermost.contains(span)) {
                outermost.push(span);
            }
        }
        for span in outermost {
            cx.report(span);
        }
    }
}
