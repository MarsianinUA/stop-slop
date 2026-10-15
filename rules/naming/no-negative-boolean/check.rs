use std::collections::BTreeMap;

use kit::oxc::semantic::{ScopeId, Scoping, SymbolFlags, SymbolId};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// A word and its opposite (`Active` -> `Inactive`); the word is looked up without case.
    antonyms: BTreeMap<String, String>,
}

/// A prefix and the words that negate it right after it. `is` + `No` is mostly a term
/// (`isNoScriptError` for Redis `NOSCRIPT`).
const NEGATIONS: &[(&str, &[&str])] = &[
    ("is", &["not", "non"]),
    ("has", &["no", "not"]),
    ("can", &["not"]),
    ("should", &["not"]),
];

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
        if is_negative(name) {
            cx.report(span).arg("name", name);
        }
    }

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let scoping = file.semantic.scoping();
        let nodes = file.semantic.nodes();
        for symbol in scoping.symbol_ids() {
            let name = scoping.symbol_name(symbol);
            // A class `IsNotFoundError` or a type is not a boolean, and a destructured name
            // (`const { isNotValid } = props`) comes from its key.
            let is_value = match nodes.kind(scoping.symbol_declaration(symbol)) {
                AstKind::VariableDeclarator(declarator) => declarator.id.is_binding_identifier(),
                AstKind::FormalParameter(parameter) => parameter.pattern.is_binding_identifier(),
                AstKind::FormalParameterRest(parameter) => {
                    parameter.rest.argument.is_binding_identifier()
                }
                AstKind::Function(_) => true,
                _ => false,
            };
            if !is_value || !is_negative(name) || !is_own_name(file, symbol) {
                continue;
            }
            let edits = self
                .positive(name)
                .filter(|positive| !self.has_clashing_neighbour(file, symbol, positive))
                .and_then(|positive| rename(file, symbol, &positive));
            cx.report(scoping.symbol_span(symbol))
                .arg("name", name)
                .fix(edits.unwrap_or_default());
        }
    }
}

impl Check {
    /// `isNotActive` -> `isInactive`, `IS_NOT_VISIBLE` -> `IS_HIDDEN`. Only `is` + `Not` + a word
    /// from `antonyms`, in a name of one case: `is_not_active` waits for `naming-case`, `hasNoRows`
    /// needs the positive flag negated where it is read, not a rename, and a longer name may hold
    /// a term (`isNotFoundError` is not `isMissingError`).
    fn positive(&self, name: &str) -> Option<String> {
        let constant = Case::Constant.matches(name);
        if !constant && !Case::Camel.matches(name) && !Case::Pascal.matches(name) {
            return None;
        }
        let words = words(name);
        let [is, not, word] = words.as_slice() else {
            return None;
        };
        if !is.eq_ignore_ascii_case("is") || !not.eq_ignore_ascii_case("not") {
            return None;
        }
        let antonym = self
            .antonyms
            .iter()
            .find_map(|(key, antonym)| key.eq_ignore_ascii_case(word).then_some(antonym))?;
        let antonym = if word.chars().any(char::is_lowercase) {
            antonym.clone()
        } else {
            antonym.to_uppercase()
        };
        // `is` or `IS_`, as written.
        let head = &name[..is.len() + usize::from(constant)];
        Some(format!("{head}{antonym}"))
    }

    /// A value in a related scope that may take the name `positive` too in the same pass: one
    /// named `positive` without its first word, ignoring `_`, `-`, `$` and case, which
    /// `boolean-prefix` may rename (`disabled` while `isNotEnabled` becomes `isDisabled`), or
    /// another negative name with the same opposite (`isNotSame` next to `isNotEqual`, both
    /// `isDifferent`).
    fn has_clashing_neighbour(&self, file: &File<'_>, symbol: SymbolId, positive: &str) -> bool {
        let scoping = file.semantic.scoping();
        let home = scoping.symbol_scope_id(symbol);
        let name = scoping.symbol_name(symbol);
        let unprefixed: String = loose(&words(positive)[1..].concat()).collect();
        scoping.symbol_ids().any(|other| {
            let other_name = scoping.symbol_name(other);
            other_name != name
                && scoping.symbol_flags(other).intersects(SymbolFlags::Value)
                && is_related(scoping, scoping.symbol_scope_id(other), home)
                && (loose(other_name).eq(unprefixed.chars())
                    || self
                        .positive(other_name)
                        .is_some_and(|other| loose(&other).eq(loose(positive))))
        })
    }
}

fn is_negative(name: &str) -> bool {
    let words = words(name);
    let [prefix, negation, _, ..] = words.as_slice() else {
        return false;
    };
    NEGATIONS.iter().any(|(word, negations)| {
        prefix.eq_ignore_ascii_case(word)
            && negations
                .iter()
                .any(|negative| negation.eq_ignore_ascii_case(negative))
    })
}

fn loose(name: &str) -> impl Iterator<Item = char> + '_ {
    name.chars()
        .filter(|ch| !matches!(ch, '_' | '-' | '$'))
        .flat_map(char::to_lowercase)
}

/// One scope is the other or nested in it.
fn is_related(scoping: &Scoping, a: ScopeId, b: ScopeId) -> bool {
    scoping.scope_ancestors(a).any(|scope| scope == b)
        || scoping.scope_ancestors(b).any(|scope| scope == a)
}
