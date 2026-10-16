use kit::oxc::ast::ast::{ImportDeclaration, Statement};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

const MOVE: &str = "Move it into its group.";
const VALUES: &str = "Move it by hand, and check the load order first: value imports run their modules in the order written, so a module that must run before the others (telemetry, env setup) belongs in a side-effect import (`import './telemetry'`), which keeps its place.";
const COMMENTS: &str = "Move it by hand: a comment, a lint directive or other code in this block may belong with a neighbor.";

impl Rule for Check {
    const USES: &'static [Fact] = &[Fact::Packages, Fact::Aliases];

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let program = file.semantic.nodes().program();
        let runs = program
            .body
            .chunk_by(|a, b| with_bindings(a).is_some() && with_bindings(b).is_some());
        for run in runs {
            let Some(block) = run.iter().map(with_bindings).collect::<Option<Vec<_>>>() else {
                continue;
            };
            let groups: Vec<u8> = block
                .iter()
                .map(|import| group(import_kind(file, cx.facts(), &import.source.value)))
                .collect();
            if groups.is_sorted() {
                continue;
            }
            // Name the smaller set: the imports below a later group, or those above an
            // earlier one. One `import type` that sits too high is one import, not the rest.
            let count = groups.len();
            let late: Vec<(usize, usize)> = (0..count)
                .filter_map(|index| {
                    (0..index)
                        .find(|&other| groups[other] > groups[index])
                        .map(|other| (index, other))
                })
                .collect();
            let early: Vec<(usize, usize)> = (0..count)
                .filter_map(|index| {
                    (index + 1..count)
                        .min_by_key(|&other| groups[other])
                        .filter(|&other| groups[other] < groups[index])
                        .map(|other| (index, other))
                })
                .collect();
            let ((index, other), place) = if early.len() < late.len() {
                (early[0], "before")
            } else {
                (late[0], "after")
            };
            let fix = reorder(file, &block, &groups);
            cx.report(block[index].span)
                .arg("source", block[index].source.value.as_str())
                .arg("group", group_name(groups[index]))
                .arg("where", place)
                .arg("other", block[other].source.value.as_str())
                .arg("other_group", group_name(groups[other]))
                .arg("hint", fix.as_ref().err().copied().unwrap_or(MOVE))
                .fix(fix.ok());
        }
    }
}

/// Same groups as `kit::add_import`.
fn group(kind: ImportKind) -> u8 {
    match kind {
        ImportKind::Node => 0,
        ImportKind::External => 1,
        ImportKind::Workspace | ImportKind::Alias => 2,
        ImportKind::Relative => 3,
    }
}

fn group_name(group: u8) -> &'static str {
    match group {
        0 => "a Node built-in",
        1 => "an external package",
        2 => "a workspace package or alias",
        _ => "a relative path",
    }
}

/// An import this rule sorts, as in `kit::add_import`. `import 'x'`, `import {} from 'x'` and
/// `import defer` / `import source` load a module for its effect where they stand, so they keep
/// their place and end a block.
fn with_bindings<'a>(statement: &'a Statement<'a>) -> Option<&'a ImportDeclaration<'a>> {
    match statement {
        Statement::ImportDeclaration(import)
            if import.phase.is_none()
                && import
                    .specifiers
                    .as_ref()
                    .is_some_and(|list| !list.is_empty()) =>
        {
            Some(import)
        }
        _ => None,
    }
}

/// The block sorted by group, stable within one, as one edit; `Err` with the hint when the
/// new order could change what loads first or which import a comment speaks of.
fn reorder(
    file: &File<'_>,
    block: &[&ImportDeclaration<'_>],
    groups: &[u8],
) -> Result<Edit, &'static str> {
    let text = file.text;
    // TypeScript erases `import type`, so only those may move.
    let values_in_order = block
        .iter()
        .zip(groups)
        .filter(|(import, _)| import.import_kind.is_value())
        .map(|(_, &group)| group)
        .is_sorted();
    if !values_in_order {
        return Err(VALUES);
    }
    let start = block[0].span.start;
    // A comment above the first import stays where it is: the file's header, or a note on that
    // import, which would then head another one.
    let first_moves = groups[1..].iter().any(|&group| group < groups[0]);
    if first_moves && has_comment_above(file, start) {
        return Err(COMMENTS);
    }

    // Each import with the comments on the lines right above it and the rest of its line.
    let mut contents = Vec::with_capacity(block.len());
    let mut end = start;
    for import in block {
        let content = file
            .semantic
            .comments_range(end..import.span.start)
            .next()
            .map_or(import.span.start, |comment| comment.span.start);
        // A comment set off by a blank line heads a section, not the import under it.
        if has_blank_line(&text[content as usize..import.span.start as usize]) {
            return Err(COMMENTS);
        }
        // Code on the line, or a comment about the next line (`// eslint-disable-next-line`).
        let (tail, _) = line_tail(file, import.span.end).ok_or(COMMENTS)?;
        contents.push(Span::new(content, tail));
        end = tail;
    }
    if file
        .semantic
        .comments_range(start..end)
        .any(|comment| opens_region(comment.span.source_text(text)))
    {
        return Err(COMMENTS);
    }

    let gap = |above: usize, below: usize| {
        &text[contents[above].end as usize..contents[below].start as usize]
    };
    let spaced = (1..contents.len()).any(|index| has_blank_line(gap(index - 1, index)));
    let mut order: Vec<usize> = (0..contents.len()).collect();
    order.sort_by_key(|&index| groups[index]);
    let mut out = String::with_capacity((end - start) as usize);
    out.push_str(contents[order[0]].source_text(text));
    // A pair that stood together keeps its gap, so only the moved imports change their lines.
    for pair in order.windows(2) {
        let (previous, index) = (pair[0], pair[1]);
        if previous + 1 == index {
            out.push_str(gap(previous, index));
        } else if spaced && groups[previous] != groups[index] {
            out.push_str("\n\n");
        } else {
            out.push('\n');
        }
        out.push_str(contents[index].source_text(text));
    }
    Ok(Edit {
        span: Span::new(start, end),
        text: out,
    })
}

/// The last comment before `start` stands alone on the lines right above it.
fn has_comment_above(file: &File<'_>, start: u32) -> bool {
    let text = file.text;
    file.semantic
        .comments_range(..start)
        .next_back()
        .is_some_and(|comment| {
            let between = &text[comment.span.end as usize..start as usize];
            let line = &text[..comment.span.start as usize];
            let before = &line[line.rfind('\n').map_or(0, |index| index + 1)..];
            between.trim().is_empty() && !has_blank_line(between) && before.trim().is_empty()
        })
}

/// `eslint-disable` and `eslint-enable` bound the imports between them, and `#region` and
/// `#endregion` fold them; the `-line` and `-next-line` forms move with their import.
fn opens_region(comment: &str) -> bool {
    comment.contains("#region")
        || comment.contains("#endregion")
        || comment.contains("eslint-enable")
        || comment
            .split("eslint-disable")
            .skip(1)
            .any(|rest| !rest.starts_with("-line") && !rest.starts_with("-next-line"))
}

/// A line of `text` other than its first and last holds only spaces.
fn has_blank_line(text: &str) -> bool {
    let mut lines = text.split('\n');
    lines.next();
    lines.next_back();
    lines.any(|line| line.trim().is_empty())
}
