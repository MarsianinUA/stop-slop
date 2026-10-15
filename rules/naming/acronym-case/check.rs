use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// In capitals. A run of more than three capitals is fixed only when it is one of them or a
    /// chain of them: `HTTPURL` -> `HttpUrl`, while in `ETHUSD` the word boundary is a guess.
    acronyms: Vec<String>,
}

impl Rule for Check {
    const NODES: &'static [AstType] = &[
        AstType::PropertyDefinition,
        AstType::MethodDefinition,
        AstType::AccessorProperty,
        AstType::FormalParameter,
    ];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let Some((name, span)) = private_member(cx.file(), node) else {
            return;
        };
        // A member is not a symbol: `this.x` here and `obj['x']` elsewhere are out of reach.
        if let Some((suggestion, _)) = self.rewrite(name) {
            cx.report(span)
                .arg("name", name)
                .arg("suggestion", suggestion);
        }
    }

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let scoping = file.semantic.scoping();
        for symbol in scoping.symbol_ids() {
            let name = scoping.symbol_name(symbol);
            let Some((suggestion, exact)) = self.rewrite(name) else {
                continue;
            };
            if !is_own_name(file, symbol) {
                continue;
            }
            let edits = exact.then(|| rename(file, symbol, &suggestion)).flatten();
            cx.report(scoping.symbol_span(symbol))
                .arg("name", name)
                .arg("suggestion", &suggestion)
                .fix(edits.unwrap_or_default());
        }
    }
}

impl Check {
    /// `name` with each acronym written as a word, and whether every word boundary in it is
    /// certain; `None` when it has no acronym.
    fn rewrite(&self, name: &str) -> Option<(String, bool)> {
        let bare = name.trim_start_matches('_');
        // A `_` inside is `naming-case`'s, and a name all in capitals is a constant (`MAX_ID`)
        // or one acronym (`type ID`).
        if !(Case::Camel.matches(bare) || Case::Pascal.matches(bare))
            || Case::Constant.matches(bare)
        {
            return None;
        }
        let mut rewritten = name[..name.len() - bare.len()].to_string();
        let mut exact = true;
        for word in words(bare) {
            // Digits and a plural `s` follow the capitals: `ERC20`, `IDs`.
            let run = word
                .strip_suffix('s')
                .unwrap_or(word)
                .trim_end_matches(|ch: char| ch.is_ascii_digit());
            if run.chars().count() < 2 || !run.chars().all(char::is_uppercase) {
                rewritten.push_str(word);
                continue;
            }
            let spelled = self.spell(run);
            exact &= spelled.is_some();
            rewritten.push_str(&spelled.unwrap_or_else(|| as_word(run)));
            rewritten.push_str(&word[run.len()..]);
        }
        (rewritten != name).then_some((rewritten, exact))
    }

    /// The run of capitals written as words; `None` when where its words part is a guess. Two or
    /// three capitals are one acronym; a longer run must read one way only as a chain of
    /// `acronyms`.
    fn spell(&self, run: &str) -> Option<String> {
        if run.chars().count() <= 3 {
            return Some(as_word(run));
        }
        // `spellings[at]`: the ways to write `run[at..]` as a chain of `acronyms`, built from the
        // end. Two ways are as good as many, so each list stops at two: overlapping entries
        // (`A`, `AA`) would otherwise multiply the ways with every letter.
        let mut spellings = vec![Vec::new(); run.len() + 1];
        spellings[run.len()].push(String::new());
        for at in (0..run.len()).rev().filter(|&at| run.is_char_boundary(at)) {
            let mut ways: Vec<String> = Vec::new();
            for acronym in &self.acronyms {
                // An empty entry reads `spellings[at]`, still empty here.
                if !run[at..].starts_with(acronym.as_str()) {
                    continue;
                }
                for tail in &spellings[at + acronym.len()] {
                    let way = as_word(acronym) + tail;
                    if ways.len() < 2 && !ways.contains(&way) {
                        ways.push(way);
                    }
                }
            }
            spellings[at] = ways;
        }
        match <[String; 1]>::try_from(spellings.swap_remove(0)) {
            Ok([spelling]) => Some(spelling),
            Err(_) => None,
        }
    }
}

/// `HTTP` -> `Http`.
fn as_word(run: &str) -> String {
    Case::Pascal.apply(run).unwrap_or_default()
}
