use kit::oxc::ast::ast::{
    BinaryExpression, BinaryOperator, Expression, IdentifierReference, IfStatement,
    LogicalOperator, Statement, UnaryOperator, VariableDeclarationKind,
};
use kit::oxc::semantic::SymbolId;
use kit::oxc::span::ContentEq;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::IdentifierReference, AstType::BinaryExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let found = match node.kind() {
            AstKind::IdentifierReference(id) => truthy(file, id),
            AstKind::BinaryExpression(binary) => compared(file, binary),
            _ => None,
        };
        let Some((test, negated)) = found else {
            return;
        };
        let Some(flipped) = decides(file, node) else {
            return;
        };
        let Some(proof) = proven(file, node, &test, negated != flipped) else {
            return;
        };
        let value = proof.holds != negated;
        let value = match (test.form, value) {
            (Form::Truthy, true) => "truthy",
            (Form::Truthy, false) => "falsy",
            (Form::Equals { .. }, true) => "true",
            (Form::Equals { .. }, false) => "false",
        };
        cx.report(node.span())
            .arg("condition", one_line(node.span().source_text(file.text)))
            .arg("value", value)
            .arg("check", proof.check)
            .arg("line", file.lines.position(proof.at.start).line)
            .arg("name", test.name)
            .arg(
                "binding",
                if test.parameter {
                    "a parameter nothing reassigns"
                } else {
                    "a `const`"
                },
            );
    }
}

/// A test only a stable binding decides: a `const`, or a parameter nothing writes.
struct Test<'a> {
    symbol: SymbolId,
    name: &'a str,
    parameter: bool,
    form: Form<'a>,
}

#[derive(Clone, Copy)]
enum Form<'a> {
    /// `user`.
    Truthy,
    /// `status === 'paid'`, `typeof value === 'string'`, `id == null`. `!==` and `!=` are the
    /// same test, negated.
    Equals {
        of_type: bool,
        strict: bool,
        literal: &'a Expression<'a>,
    },
}

impl Test<'_> {
    fn is(&self, other: &Test<'_>) -> bool {
        self.symbol == other.symbol
            && match (self.form, other.form) {
                (Form::Truthy, Form::Truthy) => true,
                (
                    Form::Equals {
                        of_type,
                        strict,
                        literal,
                    },
                    Form::Equals {
                        of_type: other_type,
                        strict: other_strict,
                        literal: other_literal,
                    },
                ) => {
                    of_type == other_type
                        && strict == other_strict
                        && literal.content_eq(other_literal)
                }
                _ => false,
            }
    }
}

/// `(test, negated)` of an expression.
fn test_of<'a>(file: &File<'a>, expression: &'a Expression<'a>) -> Option<(Test<'a>, bool)> {
    match expression {
        Expression::Identifier(id) => truthy(file, id),
        Expression::BinaryExpression(binary) => compared(file, binary),
        _ => None,
    }
}

fn truthy<'a>(file: &File<'a>, id: &'a IdentifierReference<'a>) -> Option<(Test<'a>, bool)> {
    let (symbol, parameter) = stable(file, id)?;
    let test = Test {
        symbol,
        name: id.name.as_str(),
        parameter,
        form: Form::Truthy,
    };
    Some((test, false))
}

/// `x === 'a'`, `'a' !== x`, `typeof x === 'string'`; `==` and `!=` only with `null` or
/// `undefined`, which call no `valueOf`.
fn compared<'a>(file: &File<'a>, binary: &'a BinaryExpression<'a>) -> Option<(Test<'a>, bool)> {
    let (strict, negated) = match binary.operator {
        BinaryOperator::StrictEquality => (true, false),
        BinaryOperator::StrictInequality => (true, true),
        BinaryOperator::Equality => (false, false),
        BinaryOperator::Inequality => (false, true),
        _ => return None,
    };
    let (left, right) = (
        binary.left.get_inner_expression(),
        binary.right.get_inner_expression(),
    );
    let (subject, literal) = if is_literal(file, right) {
        (left, right)
    } else if is_literal(file, left) {
        (right, left)
    } else {
        return None;
    };
    if !strict && !is_nullish(file, literal) {
        return None;
    }
    let (id, of_type) = match subject {
        Expression::Identifier(id) => (id, false),
        Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::Typeof => {
            let Expression::Identifier(id) = unary.argument.get_inner_expression() else {
                return None;
            };
            (id, true)
        }
        _ => return None,
    };
    let (symbol, parameter) = stable(file, id)?;
    let form = Form::Equals {
        of_type,
        strict,
        literal,
    };
    let test = Test {
        symbol,
        name: id.name.as_str(),
        parameter,
        form,
    };
    Some((test, negated))
}

fn is_literal(file: &File<'_>, expression: &Expression<'_>) -> bool {
    match expression {
        Expression::StringLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_) => true,
        Expression::TemplateLiteral(template) => template.expressions.is_empty(),
        Expression::UnaryExpression(unary) => {
            unary.operator == UnaryOperator::UnaryNegation
                && matches!(
                    unary.argument,
                    Expression::NumericLiteral(_) | Expression::BigIntLiteral(_)
                )
        }
        expression => is_global(file, expression, "undefined"),
    }
}

fn is_nullish(file: &File<'_>, expression: &Expression<'_>) -> bool {
    matches!(expression, Expression::NullLiteral(_)) || is_global(file, expression, "undefined")
}

/// The symbol and whether it is a parameter, when nothing can change what the name holds.
fn stable(file: &File<'_>, id: &IdentifierReference<'_>) -> Option<(SymbolId, bool)> {
    let scoping = file.semantic.scoping();
    let symbol = scoping.get_reference(id.reference_id()).symbol_id()?;
    let nodes = file.semantic.nodes();
    let declaration = scoping.symbol_declaration(symbol);
    let parameter = match nodes.kind(declaration) {
        AstKind::FormalParameter(_) | AstKind::FormalParameterRest(_) => true,
        AstKind::VariableDeclarator(_) => match nodes.parent_kind(declaration) {
            AstKind::VariableDeclaration(list) if list.kind == VariableDeclarationKind::Const => {
                false
            }
            _ => return None,
        },
        _ => return None,
    };
    let written = scoping
        .get_resolved_references(symbol)
        .any(|reference| reference.is_write());
    (!written).then_some((symbol, parameter))
}

/// Whether the value of the node decides a branch: the test of `if`, `?:` or a loop, or the
/// left side of `&&` or `||`, also through `!` and the right side of such an operator. `Some`
/// says whether an odd number of `!` stands between. `??` tests for `null`, not for truth.
fn decides(file: &File<'_>, node: &AstNode<'_>) -> Option<bool> {
    let mut child = node.span();
    let mut flipped = false;
    for parent in file.semantic.nodes().ancestors(node.id()) {
        let test = match parent.kind() {
            AstKind::UnaryExpression(unary) if unary.operator == UnaryOperator::LogicalNot => {
                flipped = !flipped;
                None
            }
            AstKind::TSAsExpression(_)
            | AstKind::TSSatisfiesExpression(_)
            | AstKind::TSNonNullExpression(_)
            | AstKind::TSTypeAssertion(_) => None,
            AstKind::LogicalExpression(logical)
                if logical.operator != LogicalOperator::Coalesce =>
            {
                (logical.left.span() == child).then_some(true)
            }
            AstKind::IfStatement(statement) => Some(statement.test.span() == child),
            AstKind::ConditionalExpression(conditional) => Some(conditional.test.span() == child),
            AstKind::WhileStatement(statement) => Some(statement.test.span() == child),
            AstKind::DoWhileStatement(statement) => Some(statement.test.span() == child),
            AstKind::ForStatement(statement) => Some(
                statement
                    .test
                    .as_ref()
                    .is_some_and(|test| test.span() == child),
            ),
            _ => Some(false),
        };
        match test {
            Some(true) => return Some(flipped),
            Some(false) => return None,
            None => child = parent.span(),
        }
    }
    None
}

struct Proof {
    /// The value the test of the node, without its own `!==`, has here.
    holds: bool,
    /// "the `if`", "the `&&`".
    check: &'static str,
    /// The expression that proved it.
    at: Span,
}

/// What a test around the node already proved about `test`, the innermost first. `polarity` is
/// the number of negations, odd or even, between `test` and the decision it is part of. The
/// walk stops at a function: TypeScript before 5.4 forgets a narrowed parameter inside a
/// callback, and a function declaration forgets even a narrowed `const`.
fn proven<'a>(
    file: &File<'a>,
    node: &AstNode<'a>,
    test: &Test<'_>,
    polarity: bool,
) -> Option<Proof> {
    let mut child = node.span();
    for ancestor in file.semantic.nodes().ancestors(node.id()) {
        let source = match ancestor.kind() {
            AstKind::IfStatement(statement) if statement.consequent.span() == child => {
                Some((&statement.test, true, "the `if`", false))
            }
            AstKind::IfStatement(statement)
                if statement
                    .alternate
                    .as_ref()
                    .is_some_and(|alternate| alternate.span() == child) =>
            {
                let chained = in_chain_test(statement, node.span());
                Some((&statement.test, false, "the `if`", chained))
            }
            AstKind::ConditionalExpression(conditional)
                if conditional.consequent.span() == child =>
            {
                Some((&conditional.test, true, "the `?:`", false))
            }
            AstKind::ConditionalExpression(conditional)
                if conditional.alternate.span() == child =>
            {
                Some((&conditional.test, false, "the `?:`", false))
            }
            AstKind::LogicalExpression(logical) if logical.right.span() == child => {
                match logical.operator {
                    LogicalOperator::And => Some((&logical.left, true, "the `&&`", false)),
                    LogicalOperator::Or => Some((&logical.left, false, "the `||`", false)),
                    LogicalOperator::Coalesce => None,
                }
            }
            AstKind::Function(_) | AstKind::ArrowFunctionExpression(_) => return None,
            _ => None,
        };
        if let Some((expression, value, check, chained)) = source
            && let Some(holds) = implied(file, expression, value, test)
            // An `else if` test that comes out false repeats an earlier test of its chain:
            // ESLint's `no-dupe-else-if` reports that.
            && !(chained && holds == polarity)
        {
            return Some(Proof {
                holds,
                check,
                at: expression.span(),
            });
        }
        child = ancestor.span();
    }
    None
}

/// The node sits in the test of an `else if` of the chain that `statement` starts.
fn in_chain_test(statement: &IfStatement<'_>, node: Span) -> bool {
    let mut next = statement.alternate.as_ref();
    while let Some(Statement::IfStatement(inner)) = next {
        let test = inner.test.span();
        if test.start <= node.start && node.end <= test.end {
            return true;
        }
        next = inner.alternate.as_ref();
    }
    false
}

/// The value of `test` (without its own negation) when `expression` has `value`: `a && b` true
/// proves `a` and `b`, `a || b` false proves both false, `!a` flips.
fn implied<'a>(
    file: &File<'a>,
    expression: &'a Expression<'a>,
    value: bool,
    test: &Test<'_>,
) -> Option<bool> {
    match expression.get_inner_expression() {
        Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::LogicalNot => {
            implied(file, &unary.argument, !value, test)
        }
        Expression::LogicalExpression(logical)
            if (logical.operator == LogicalOperator::And && value)
                || (logical.operator == LogicalOperator::Or && !value) =>
        {
            implied(file, &logical.left, value, test)
                .or_else(|| implied(file, &logical.right, value, test))
        }
        expression => {
            let (found, negated) = test_of(file, expression)?;
            found.is(test).then_some(value != negated)
        }
    }
}
