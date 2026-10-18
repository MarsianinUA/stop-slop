use std::borrow::Cow;
use std::collections::BTreeMap;

use kit::oxc::allocator::Allocator;
use kit::oxc::span::SourceType;
use kit::{Edit, Facts, LineIndex, RuleMeta};

use crate::run::parse;
use crate::{Engine, FixCount, Mode, Rewritten, SourceFile, finding, text};

/// Passes that apply edits; one more pass only parses the result.
const MAX_PASSES: usize = 3;

impl Engine {
    /// The new bytes of `file` and how many findings of each rule were fixed, sorted by rule;
    /// `None` when nothing changes. A text that does not parse, before or after any pass,
    /// leaves the whole file as it was.
    pub(crate) fn fix_file(
        &self,
        allocator: &mut Allocator,
        file: &SourceFile,
        facts: &dyn Facts,
    ) -> Option<(Rewritten, Vec<FixCount>)> {
        let role = self.set.roles.role_of(&file.path);
        let profile = self
            .dispatch
            .profile(Mode::Fix, role, facts.package(&file.path));
        if profile.rules.is_empty() {
            return None;
        }
        let (original, bom) = text::decode(&file.bytes)?;
        let source_type = SourceType::from_path(&file.path).ok()?;
        let ending = LineIndex::new(original).line_ending();

        let mut current = Cow::Borrowed(original);
        let mut counts: BTreeMap<&'static str, u32> = BTreeMap::new();
        for pass in 0..=MAX_PASSES {
            allocator.reset();
            let program = parse(allocator, &current, source_type).ok()?;
            if pass == MAX_PASSES {
                break;
            }
            let mut fixes = Vec::new();
            self.run_rules(
                &program,
                &file.path,
                role,
                profile,
                facts,
                |source, meta, report, function| {
                    let Some(edits) = sound_edits(meta, report.edits, source.text, ending) else {
                        return;
                    };
                    let place = report.span.source_text(source.text);
                    let key = finding::ignore_key(&file.path, meta.id, place, function);
                    if !self.ignored.contains(&key) {
                        fixes.push((meta.id, edits));
                    }
                },
            );
            let taken = choose(fixes);
            if taken.is_empty() {
                break;
            }
            let mut edits = Vec::new();
            for (rule, fix) in taken {
                *counts.entry(rule).or_default() += 1;
                edits.extend(fix);
            }
            current = Cow::Owned(apply(&current, edits, ending));
        }
        if current == original {
            return None;
        }

        let fixed = counts
            .into_iter()
            .map(|(rule, count)| FixCount {
                rule,
                file: file.path.clone(),
                count,
            })
            .collect();
        let rewritten = Rewritten {
            path: file.path.clone(),
            bytes: text::encode(current.into_owned(), bom),
        };
        Some((rewritten, fixed))
    }
}

/// The edits `Fix` applies for a report, sorted, without those that leave `text` as it is.
/// `None` when the rule has `fix = false`, when an edit lies outside `text`, cuts a char or
/// overlaps the next one, or when no edit is left: that finding stays unfixed, and `Check`
/// does not mark it fixable.
pub(crate) fn sound_edits(
    meta: &RuleMeta,
    mut edits: Vec<Edit>,
    text: &str,
    ending: &str,
) -> Option<Vec<Edit>> {
    if !meta.fix {
        return None;
    }
    edits.sort_by_key(|edit| (edit.span.start, edit.span.end));
    let inside = |offset: u32| text.is_char_boundary(offset as usize);
    let sound = edits.iter().all(|edit| {
        edit.span.start <= edit.span.end && inside(edit.span.start) && inside(edit.span.end)
    }) && edits
        .windows(2)
        .all(|pair| pair[0].span.end <= pair[1].span.start);
    if !sound {
        return None;
    }
    // Taken in every pass, an edit that changes nothing would count again each time and hold
    // back the fixes it touches.
    edits.retain(|edit| {
        text[edit.span.start as usize..edit.span.end as usize]
            != *text::with_line_ending(&edit.text, ending)
    });
    (!edits.is_empty()).then_some(edits)
}

/// The fixes of one pass: in order of their first edit, then as given, each fix whose edits
/// neither overlap nor touch the edits of one already taken. Touching edits wait for the
/// next pass, so the order of two inserts at one point is never guessed.
fn choose(mut fixes: Vec<(&'static str, Vec<Edit>)>) -> Vec<(&'static str, Vec<Edit>)> {
    fixes.sort_by_key(|(_, edits)| edits[0].span.start);

    let touches = |a: &Edit, b: &Edit| a.span.start <= b.span.end && b.span.start <= a.span.end;
    let mut taken: Vec<(&str, Vec<Edit>)> = Vec::new();
    for (r, edits) in fixes {
        let free = edits.iter().all(|edit| {
            taken
                .iter()
                .flat_map(|(_, other)| other)
                .all(|other| !touches(edit, other))
        });
        if free {
            taken.push((r, edits));
        }
    }
    taken
}

/// `edits` of different fixes do not touch; the edits of one fix keep their sorted order.
fn apply(text: &str, mut edits: Vec<Edit>, ending: &str) -> String {
    edits.sort_by_key(|edit| (edit.span.start, edit.span.end));
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for edit in &edits {
        out.push_str(&text[at..edit.span.start as usize]);
        out.push_str(&text::with_line_ending(&edit.text, ending));
        at = edit.span.end as usize;
    }
    out.push_str(&text[at..]);
    out
}
