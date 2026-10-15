use kit::oxc::ast::ast::TSEnumMemberName;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::TSEnumDeclaration];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::TSEnumDeclaration(declaration) = node.kind() else {
            return;
        };
        let Some(symbol) = declaration.id.symbol_id.get() else {
            return;
        };
        let file = cx.file();
        if !is_own_name(file, symbol) {
            return;
        }
        let enum_name = declaration.id.name.as_str();
        let enum_words = words(enum_name);
        for member in &declaration.body.members {
            let TSEnumMemberName::Identifier(id) = &member.id else {
                continue;
            };
            let name = id.name.as_str();
            let Some(suggestion) = without_enum(&enum_words, name) else {
                continue;
            };
            cx.report(id.span)
                .arg("name", name)
                .arg("enum", enum_name)
                .arg("suggestion", suggestion);
        }
    }
}

/// `member` without the words that repeat the enum's, or `None` when it repeats nothing: it
/// starts with all of them (`StatusActive` in `Status`), ends with all of them (`DarkTheme` in
/// `Theme`), or starts with the last one, the head noun (`StatusRefunded` in `OrderStatus`).
/// A partial match (`ApiKeyRevoked` in `ApiKeyErrorCode`) may name a subject, not repeat.
fn without_enum(enum_words: &[&str], member: &str) -> Option<String> {
    let head = enum_words.last()?;
    let member_words = words(member);
    if member_words.len() < 2 {
        return None;
    }
    let longer = member_words.len() > enum_words.len();
    let same = |(a, b): (&&str, &&str)| is_same_word(a, b);
    let kept = if longer && enum_words.iter().zip(&member_words).all(same) {
        &member_words[enum_words.len()..]
    } else if longer
        && enum_words
            .iter()
            .rev()
            .zip(member_words.iter().rev())
            .all(same)
    {
        &member_words[..member_words.len() - enum_words.len()]
    } else if is_same_word(head, member_words[0]) {
        &member_words[1..]
    } else {
        return None;
    };
    // In the case `enum-member-case` asks for, so `ORDER_STATUS_PAID` gets one answer, `Paid`.
    // `Code_404` gets none: `404` is not a name.
    Case::Pascal
        .apply(&kept.join("_"))
        .filter(|name| name.starts_with(char::is_alphabetic))
}

/// The same word, case aside, also with a plural `s` on one side (`Roles` and `Role`).
fn is_same_word(a: &str, b: &str) -> bool {
    let same = |a: &str, b: &str| {
        a.chars()
            .flat_map(char::to_lowercase)
            .eq(b.chars().flat_map(char::to_lowercase))
    };
    let is_plural_of = |plural: &str, word: &str| {
        plural
            .strip_suffix(['s', 'S'])
            .is_some_and(|singular| same(singular, word))
    };
    same(a, b) || is_plural_of(a, b) || is_plural_of(b, a)
}
