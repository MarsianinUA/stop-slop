use kit::oxc::ast::ast::{Class, ClassElement, MethodDefinitionKind, PropertyKey};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::Class];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::Class(class) = node.kind() else {
            return;
        };
        let file = cx.file();
        if is_ambient(file, node.id()) {
            return;
        }
        let mut highest = Category::Field;
        let misplaced = class.body.body.iter().find_map(|element| {
            let category = category(element);
            let found = (category < highest).then_some((element, highest));
            highest = highest.max(category);
            found
        });
        let Some((element, after)) = misplaced else {
            return;
        };
        let (name, member) = describe(file, element);
        let after = match after {
            Category::Constructor => "the constructor",
            _ => "a method",
        };
        let reported = cx
            .report(place(file, element))
            .arg("name", name)
            .arg("member", member)
            .arg("after", after);
        if let Some(edit) = reorder(file, class) {
            reported.fix([edit]);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Category {
    Field,
    Constructor,
    Method,
}

/// A static block runs with the static fields; a getter, a setter and an overload are methods.
fn category(element: &ClassElement<'_>) -> Category {
    match element.method_definition_kind() {
        Some(MethodDefinitionKind::Constructor) => Category::Constructor,
        Some(_) => Category::Method,
        None => Category::Field,
    }
}

/// A method is never out of place, so the method here is the constructor.
fn describe(file: &File<'_>, element: &ClassElement<'_>) -> (String, &'static str) {
    match element {
        ClassElement::MethodDefinition(_) => ("constructor".into(), "the constructor"),
        ClassElement::StaticBlock(_) => ("static".into(), "a static block"),
        // The span of `key: string` leaves out the brackets.
        ClassElement::TSIndexSignature(signature) => (
            format!("[{}]", signature.parameter.span.source_text(file.text)),
            "an index signature",
        ),
        ClassElement::PropertyDefinition(property) => (key_name(file, &property.key), "a field"),
        ClassElement::AccessorProperty(property) => (key_name(file, &property.key), "a field"),
    }
}

fn key_name(file: &File<'_>, key: &PropertyKey<'_>) -> String {
    match key {
        PropertyKey::PrivateIdentifier(id) => format!("#{}", id.name),
        _ => key.static_name().map_or_else(
            || format!("[{}]", key.span().source_text(file.text)),
            String::from,
        ),
    }
}

/// The member without its decorators. Two classes with a field of one name still differ by the
/// rest of the member.
fn place(file: &File<'_>, element: &ClassElement<'_>) -> Span {
    let span = element.span();
    let decorators = match element {
        ClassElement::MethodDefinition(method) => &method.decorators[..],
        ClassElement::PropertyDefinition(property) => &property.decorators[..],
        ClassElement::AccessorProperty(property) => &property.decorators[..],
        ClassElement::TSIndexSignature(_) | ClassElement::StaticBlock(_) => &[],
    };
    let start = start_after_decorators(file, span, decorators);
    let end = match element {
        ClassElement::MethodDefinition(method) => method.value.params.span.end,
        _ => span.end,
    };
    Span::new(start, end)
}

/// A member with the comments above it and the rest of its line.
struct Slot {
    /// The spaces and line breaks before `content`.
    gap: Span,
    content: Span,
    category: Category,
}

/// The members sorted by category, stable within one, as one edit over the class body; `None`
/// when the new order could change what the code does or how it parses.
fn reorder(file: &File<'_>, class: &Class<'_>) -> Option<Edit> {
    let text = file.text;
    let body = &class.body;
    let elements = &body.body;
    if file
        .semantic
        .comments_range(body.span.start..body.span.end)
        .any(|comment| opens_region(comment.span.source_text(text)))
    {
        return None;
    }
    // `accessor x` defines a prototype accessor in document order, as a method does.
    let accessor_moves = elements
        .iter()
        .skip_while(|element| category(element) == Category::Field)
        .any(|element| matches!(element, ClassElement::AccessorProperty(_)));
    let timed_order_changes = !elements
        .iter()
        .filter(|element| defined_in_order(element))
        .map(category)
        .is_sorted();
    if accessor_moves || timed_order_changes {
        return None;
    }

    let blank = |from: u32, to: u32| text[from as usize..to as usize].trim().is_empty();
    let mut slots = Vec::with_capacity(elements.len());
    // Comments on the line of `{` stay with it. Code there, or a comment about the next line
    // (`// @ts-expect-error`), gets no fix: the new first member would land on that line.
    let (start, _) = line_tail(file, body.span.start + 1)?;
    let mut end = start;
    let mut open = false;
    let mut keyword_open = false;
    for element in elements {
        let span = element.span();
        let mut content = span.start;
        let mut at = end;
        for comment in file.semantic.comments_range(end..span.start) {
            if !blank(at, comment.span.start) {
                return None;
            }
            content = content.min(comment.span.start);
            at = comment.span.end;
        }
        if !blank(at, span.start) {
            return None;
        }
        let (tail, semicolon) = line_tail(file, span.end)?;
        if !semicolon && !closes(text, element) {
            open = true;
            keyword_open |= matches!(
                span.source_text(text).rsplit(char::is_whitespace).next(),
                Some("get" | "set" | "static")
            );
        }
        slots.push(Slot {
            gap: Span::new(end, content),
            content: Span::new(content, tail),
            category: category(element),
        });
        end = tail;
    }
    // Without `;`, a member may run into the next one: `prefix = 'id'` before `*ids()` reads as
    // `'id' * ids()`, `x = 1` before `in() {}` as `1 in ()`, a field `get` before `name() {}`
    // makes a getter.
    if open
        && (keyword_open
            || elements
                .iter()
                .any(|element| continues_expression(element.span().source_text(text))))
    {
        return None;
    }

    // The new first member takes the gap of the old one, so no blank line opens the body.
    let first = (0..slots.len()).min_by_key(|&index| slots[index].category)?;
    let gap = slots[0].gap;
    slots[0].gap = slots[first].gap;
    slots[first].gap = gap;
    slots.sort_by_key(|slot| slot.category);

    let mut out = String::with_capacity((end - start) as usize);
    for slot in &slots {
        out.push_str(slot.gap.source_text(text));
        out.push_str(slot.content.source_text(text));
    }
    Some(Edit {
        span: Span::new(start, end),
        text: out,
    })
}

/// `eslint-disable` and `eslint-enable` bound the members between them, and `#region` and
/// `#endregion` fold them; the `-line` and `-next-line` forms move with their member.
fn opens_region(comment: &str) -> bool {
    comment.contains("#region")
        || comment.contains("#endregion")
        || comment.contains("eslint-enable")
        || comment
            .split("eslint-disable")
            .skip(1)
            .any(|rest| !rest.starts_with("-line") && !rest.starts_with("-next-line"))
}

fn continues_expression(member: &str) -> bool {
    member.starts_with(['[', '*'])
        || ["in", "instanceof"].iter().any(|word| {
            member.strip_prefix(word).is_some_and(|rest| {
                !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '$')
            })
        })
}

/// A decorator or a computed key runs when the class is defined, so such members keep their
/// order. Decorators on constructor parameters run with those of the class.
fn defined_in_order(element: &ClassElement<'_>) -> bool {
    let computed_key = element.computed()
        && element.property_key().is_some_and(|key| match key {
            PropertyKey::StringLiteral(_) | PropertyKey::NumericLiteral(_) => false,
            PropertyKey::TemplateLiteral(template) => !template.expressions.is_empty(),
            _ => true,
        });
    let decorated_parameter = matches!(
        element,
        ClassElement::MethodDefinition(method)
            if method.kind != MethodDefinitionKind::Constructor
                && method
                    .value
                    .params
                    .items
                    .iter()
                    .any(|parameter| !parameter.decorators.is_empty())
    );
    element.has_decorator() || computed_key || decorated_parameter
}

/// A method body and a static block end the member; a field value that ends with `}` does not
/// (`x = {}` before `[k] = 1` reads as `x = {}[k] = 1`).
fn closes(text: &str, element: &ClassElement<'_>) -> bool {
    match element {
        ClassElement::MethodDefinition(method) if method.value.body.is_some() => true,
        ClassElement::StaticBlock(_) => true,
        _ => element.span().source_text(text).ends_with(';'),
    }
}
