use kit::oxc::ast::ast::{CallExpression, Expression};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// Words that only say the test works or is a test (`works`, `test`), without case.
    vague: Vec<String>,
    /// Words that add nothing around them (`should`, `correctly`, `case`), without case. Alone
    /// they may name a case (`it('ok')` for the status `ok`); with a number they number one
    /// (`case 2`).
    filler: Vec<String>,
}

/// The functions that declare a test: Jest's and Vitest's `it` and `test`, Jasmine's `xit` and
/// `fit`, Jest's `xtest`, Mocha's `specify`.
const TESTS: [&str; 6] = ["it", "test", "xit", "fit", "xtest", "specify"];
/// Modules that export them.
const RUNNERS: [&str; 6] = [
    "vitest",
    "@jest/globals",
    "bun:test",
    "node:test",
    "@playwright/test",
    "mocha",
];
/// Members that still take the name first: `it.only(name, ..)`, `test.concurrent.skip(..)`.
const MODIFIERS: [&str; 9] = [
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
const CURRIED: [&str; 6] = ["each", "for", "skipIf", "runIf", "if", "todoIf"];

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::CallExpression(call) = node.kind() else {
            return;
        };
        let Some(title) = call
            .arguments
            .first()
            .and_then(|first| first.as_expression())
        else {
            return;
        };
        let name = match title {
            Expression::StringLiteral(literal) => literal.value.as_str(),
            Expression::TemplateLiteral(template) => match template.single_quasi() {
                Some(text) => text.as_str(),
                None => return,
            },
            _ => return,
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
    /// digits (`''`) too; a bare number (`'404'`) names a case.
    fn is_vague(&self, name: &str) -> bool {
        // `each` fills `%s`, `%d` and `$expected` from its table row, and these name the case;
        // `%#` and `$#` give only the row's index.
        let fills = name.as_bytes().windows(2).any(|pair| {
            matches!(pair, [b'%' | b'$', next] if next.is_ascii_alphabetic() || *next == b'_')
        });
        if fills {
            return false;
        }
        let mut words = Vec::new();
        let mut numbered = name.contains("%#") || name.contains("$#");
        for token in name
            .split(|ch: char| !ch.is_alphanumeric())
            .filter(|token| !token.is_empty())
        {
            let word = token.trim_end_matches(|ch: char| ch.is_ascii_digit());
            numbered |= word.len() < token.len();
            if !word.is_empty() {
                words.push(word);
            }
        }
        if words.is_empty() {
            return !numbered;
        }
        let is_in =
            |list: &[String], word: &str| list.iter().any(|item| item.eq_ignore_ascii_case(word));
        words
            .iter()
            .all(|word| is_in(&self.vague, word) || is_in(&self.filler, word))
            && (numbered || words.iter().any(|word| is_in(&self.vague, word)))
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
    member
        .static_property_name()
        .is_some_and(|name| CURRIED.contains(&name))
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
        || RUNNERS
            .iter()
            .any(|runner| imported_from(file, id, runner).is_some_and(|name| TESTS.contains(&name)))
}
