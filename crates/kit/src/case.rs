#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Case {
    /// `order-item`: lowercase letters and digits, words joined by single `-`.
    Kebab,
    /// `orderItem`: a lowercase letter or `$` first, then letters, digits, `$`.
    Camel,
    /// `OrderItem`: a capital first, then letters, digits, `$`.
    Pascal,
    /// `ORDER_ITEM`: capitals and digits, words joined by single `_`.
    Constant,
}

impl Case {
    /// Runs of capitals pass Camel and Pascal (`userID`, `HTTPServer`): acronyms are
    /// `acronym-case`'s, so one name does not get two findings for one fault.
    pub fn matches(self, name: &str) -> bool {
        let Some(first) = name.chars().next() else {
            return false;
        };
        let word_char = |ch: char| ch.is_alphanumeric() || ch == '$';
        match self {
            Case::Kebab => joined_by(name, '-', |ch| {
                ch.is_ascii_lowercase() || ch.is_ascii_digit()
            }),
            Case::Camel => (first.is_lowercase() || first == '$') && name.chars().all(word_char),
            Case::Pascal => first.is_uppercase() && name.chars().all(word_char),
            Case::Constant => {
                first.is_uppercase()
                    && joined_by(name, '_', |ch| ch.is_uppercase() || ch.is_ascii_digit())
            }
        }
    }

    /// `name` rewritten in this style from its [`words`]; `None` when it has none (`_`).
    pub fn apply(self, name: &str) -> Option<String> {
        let words = words(name);
        if words.is_empty() {
            return None;
        }
        Some(match self {
            Case::Kebab => join(&words, "-", str::to_lowercase),
            Case::Constant => join(&words, "_", str::to_uppercase),
            Case::Pascal => join(&words, "", capitalize),
            Case::Camel => words[0].to_lowercase() + &join(&words[1..], "", capitalize),
        })
    }
}

/// Non-empty words of `word_char`s joined by single `separator`s.
fn joined_by(name: &str, separator: char, word_char: fn(char) -> bool) -> bool {
    name.split(separator)
        .all(|word| !word.is_empty() && word.chars().all(word_char))
}

fn join(words: &[&str], separator: &str, style: fn(&str) -> String) -> String {
    words
        .iter()
        .map(|word| style(word))
        .collect::<Vec<_>>()
        .join(separator)
}

/// `ID` -> `Id`.
fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| {
        first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect()
    })
}

/// Split at `_` and `-`, before a capital that follows a lowercase letter or a digit, and before
/// the last capital of a run that a lowercase letter follows (`HTTPServer` -> `HTTP`, `Server`).
/// A plural `s` stays with its acronym (`userIDs` -> `user`, `IDs`); a part with no lowercase
/// letter is one word (`E2E`, `MAX`); digits stay with the word before them.
pub fn words(name: &str) -> Vec<&str> {
    let mut words = Vec::new();
    for part in name.split(['_', '-']).filter(|part| !part.is_empty()) {
        if !part.chars().any(char::is_lowercase) {
            words.push(part);
            continue;
        }
        let chars: Vec<(usize, char)> = part.char_indices().collect();
        let mut start = 0;
        for i in 1..chars.len() {
            let (at, ch) = chars[i];
            let before = chars[i - 1].1;
            let after = chars.get(i + 1).map(|&(_, ch)| ch);
            // The `s` ends a run of capitals and a word: `IDs`, `URLsMap`.
            let plural_s = after == Some('s')
                && chars
                    .get(i + 2)
                    .is_none_or(|&(_, ch)| ch.is_uppercase() || ch.is_ascii_digit());
            let starts_word = ch.is_uppercase()
                && (before.is_lowercase()
                    || before.is_ascii_digit()
                    || (before.is_uppercase()
                        && after.is_some_and(char::is_lowercase)
                        && !plural_s));
            if starts_word {
                words.push(&part[start..at]);
                start = at;
            }
        }
        words.push(&part[start..]);
    }
    words
}
