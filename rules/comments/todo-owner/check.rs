use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

const TODO: &str = "TODO";

impl Rule for Check {
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        for comment in file.semantic.comments() {
            let span = comment.content_span();
            let text = span.source_text(file.text);
            for (at, _) in text.match_indices(TODO) {
                let line = text[at + TODO.len()..].lines().next().unwrap_or("");
                if is_word(&text[..at], line) && !has_owner(line) {
                    // The rest of the line goes into the span: the fingerprint hashes the span's
                    // text, and `TODO` alone would give every TODO in a function one fingerprint.
                    let len = TODO.len() + line.trim_end().len();
                    cx.report(Span::sized(span.start + at as u32, len as u32));
                }
            }
        }
    }
}

/// `TODOS`, `MYTODO` and `TODO_LIST` are other words.
fn is_word(before: &str, after: &str) -> bool {
    let word_char = |c: char| c.is_alphanumeric() || c == '_';
    !before.chars().next_back().is_some_and(word_char)
        && !after.chars().next().is_some_and(word_char)
}

/// `(name)` right after `TODO`, on the same line: `grep "TODO("` finds them all.
fn has_owner(line: &str) -> bool {
    line.strip_prefix('(')
        .and_then(|rest| rest.split_once(')'))
        .is_some_and(|(owner, _)| !owner.trim().is_empty())
}
