use kit::oxc::ast::ast::{Argument, CallExpression, Expression};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// Words that only say the test works or is a test (`works`, `test`), case-insensitive.
    vague: Vec<String>,
    /// Words that add nothing around them (`should`, `correctly`, `case`), case-insensitive. Alone
    /// they may name a case (`it('ok')` for the status `ok`); with a number they number one
    /// (`case 2`).
    filler: Vec<String>,
}

/// The functions that declare a test: Jest's and Vitest's `it` and `test`, Jasmine's `xit` and
/// `fit`, Jest's `xtest`, Mocha's `specify`.
const TESTS: &[&str] = &["it", "test", "xit", "fit", "xtest", "specify"];
const RUNNERS: &[&str] = &[
    "vitest",
    "@jest/globals",
    "bun:test",
    "node:test",
    "@playwright/test",
    "mocha",
];
/// Runners whose default export is `test`: `import test from 'node:test'`.
const DEFAULT_TEST: &[&str] = &["node:test", "@playwright/test"];
/// Members that still take the name first: `it.only(name, ..)`, `test.concurrent.skip(..)`.
const MODIFIERS: &[&str] = &[
    "only",
    "skip",
    "todo",
    "concurrent",
    "sequential",
    "failing",
    "fails",
    "fail",
    "fixme",
];
/// Members that take a table or a condition and give back the test that takes the name:
/// `it.each(rows)(name, ..)`, `test.skipIf(ci)(name, ..)`.
const CURRIED: &[&str] = &["each", "for", "skipIf", "runIf", "if", "todoIf"];

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::CallExpression(call) = node.kind() else {
            return;
        };
        let Some(title) = call.arguments.first().and_then(Argument::as_expression) else {
            return;
        };
        let Some(name) = static_text(title) else {
            return;
        };
        let file = cx.file();
        if self.is_vague(name) && declares_test(file, call) {
            cx.report(title.span())
                .arg("name", one_line(title.span().source_text(file.text)));
        }
    }
}

impl Check {
    /// Nothing but `vague` and `filler` words and numbers, with a `vague` word or a number:
    /// `works`, `should work correctly`, `test1`, `case 2`, `case %#`. A name without letters and
    /// digits (`''`, `'%#'`) too; a bare number (`'404'`) names a case.
    fn is_vague(&self, name: &str) -> bool {
        // `each` fills `%s`, `%d` and `$expected` from its table row, and these name the case;
        // `%#`, `$#` and Vitest's `%$` give only the row's index.
        let fills = name.as_bytes().windows(2).any(|pair| {
            matches!(pair, [b'%' | b'$', next] if next.is_ascii_alphabetic() || *next == b'_')
        });
        if fills {
            return false;
        }
        let is_in =
            |list: &[String], word: &str| list.iter().any(|item| item.eq_ignore_ascii_case(word));
        let index = ["%#", "$#", "%$"]
            .iter()
            .any(|marker| name.contains(marker));
        let mut digits = false;
        let mut any_word = false;
        let mut any_vague = false;
        for token in name
            .split(|ch: char| !ch.is_alphanumeric())
            .filter(|token| !token.is_empty())
        {
            let word = token.trim_end_matches(|ch: char| ch.is_ascii_digit());
            digits |= word.len() < token.len();
            if word.is_empty() {
                continue;
            }
            any_word = true;
            if is_in(&self.vague, word) {
                any_vague = true;
            } else if !is_in(&self.filler, word) {
                return false;
            }
        }
        if any_word {
            digits || index || any_vague
        } else {
            !digits
        }
    }
}

/// The call declares a test and takes its name first: `it(..)`, `test.only(..)`,
/// `it.each(rows)(..)`, `test.each`..`(..)`.
fn declares_test(file: &File<'_>, call: &CallExpression<'_>) -> bool {
    let test = match call.callee.get_inner_expression() {
        Expression::CallExpression(table) => curried(&table.callee),
        Expression::TaggedTemplateExpression(table) => curried(&table.tag),
        callee => Some(callee),
    };
    test.is_some_and(|test| is_test(file, test))
}

/// The test behind `it.each` or `test.skipIf`.
fn curried<'b, 'a>(callee: &'b Expression<'a>) -> Option<&'b Expression<'a>> {
    let member = callee.get_member_expr()?;
    CURRIED
        .contains(&member.static_property_name()?)
        .then(|| member.object())
}

/// `it` or `test` of a runner, through `only`, `skip` and the like: the global the runner gives,
/// or an import from the runner under any name.
fn is_test(file: &File<'_>, test: &Expression<'_>) -> bool {
    let mut test = test.get_inner_expression();
    while let Some(member) = test.get_member_expr() {
        if !member
            .static_property_name()
            .is_some_and(|name| MODIFIERS.contains(&name))
        {
            return false;
        }
        test = member.object().get_inner_expression();
    }
    let Expression::Identifier(id) = test else {
        return false;
    };
    TESTS.iter().any(|name| is_global(file, test, name))
        || imported_name(file, id).is_some_and(|(module, name)| {
            let runner = package_name(module);
            (TESTS.contains(&name) && RUNNERS.contains(&runner))
                || (name == "default" && DEFAULT_TEST.contains(&runner))
        })
}
