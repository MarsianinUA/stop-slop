use oxc_ast::Comment;
use oxc_span::Span;

use crate::{File, line_start};

/// True when a `// SAFETY: <reason>` comment explains the code at `span`: a comment that starts
/// on the line where `span` ends, or one in the block of own-line comments right above the line
/// where `span` starts. The reason must not be empty.
pub fn has_safety_comment(file: &File<'_>, span: Span) -> bool {
    let text = file.text;
    let comments = file.semantic.comments();
    let is_safety = |comment: &Comment| {
        comment
            .content_span()
            .source_text(text)
            .trim_start_matches(|ch: char| ch == '*' || ch.is_whitespace())
            .strip_prefix("SAFETY:")
            .is_some_and(|reason| !reason.trim().is_empty())
    };

    let end_line = line_start(text, span.end);
    let end_line_end = text[span.end as usize..]
        .find('\n')
        .map_or(text.len(), |at| span.end as usize + at) as u32;
    let first = comments.partition_point(|comment| comment.span.start < end_line);
    if comments[first..]
        .iter()
        .take_while(|comment| comment.span.start < end_line_end)
        .any(is_safety)
    {
        return true;
    }

    // Walk up while each comment sits alone on the line right above the previous one.
    let mut below = line_start(text, span.start);
    loop {
        let index = comments.partition_point(|comment| comment.span.end <= below);
        let Some(index) = index.checked_sub(1) else {
            return false;
        };
        let comment = &comments[index];
        let Span { start, end, .. } = comment.span;
        let gap = &text[end as usize..below as usize];
        let own_line = line_start(text, start);
        if gap.matches('\n').count() != 1
            || !gap.trim().is_empty()
            || !text[own_line as usize..start as usize].trim().is_empty()
        {
            return false;
        }
        if is_safety(comment) {
            return true;
        }
        below = own_line;
    }
}
